use super::*;
use std::sync::atomic::AtomicBool;

fn workspace(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "audio-population-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("source/Nature")).unwrap();
    root
}
fn wav() -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&16036_u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&8000_u32.to_le_bytes());
    bytes.extend_from_slice(&16000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&16000_u32.to_le_bytes());
    bytes.resize(16044, 0);
    bytes
}
fn sound_query() -> CatalogQuery {
    CatalogQuery {
        kind: Some(AssetKind::Sound),
        ..Default::default()
    }
}

#[test]
fn explicit_audio_scope_freezes_all_matching_variants_before_analysis_and_ignores_pages() {
    let root = workspace("freeze");
    let original = wav();
    for i in 0..45 {
        fs::write(
            root.join(format!("source/Nature/Stream{i:03}.wav")),
            &original,
        )
        .unwrap();
        fs::write(
            root.join(format!("source/Nature/Stream{i:03}.mp3")),
            include_bytes!("../tests/fixtures/audio/stereo44100.mp3"),
        )
        .unwrap();
    }
    let mut library = Library::create(&root.join("Library")).unwrap();
    let package = library.import_folder(&root.join("source")).unwrap();
    let folder = library.create_folder("Project", None).unwrap();
    let child = library.create_folder("Sounds", Some(folder.id)).unwrap();
    let page = library
        .query(CatalogQuery {
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total, 45);
    for asset in &page.items {
        library.add_to_folder(child.id, asset.id).unwrap();
        library.add_to_folder(folder.id, asset.id).unwrap();
    }
    library
        .annotate_package(package.id, &["nature".into()])
        .unwrap();
    let ids = library
        .prepare_audio_analysis(
            CatalogQuery {
                package_id: Some(package.id),
                folder_id: Some(folder.id),
                original_folder: Some("Nature".into()),
                text: "Stream".into(),
                tag: Some("nature".into()),
                offset: 40,
                limit: 1,
                ..sound_query()
            },
            Some(AudioCriteria {
                analysis: Some("notAnalysed".into()),
                ..Default::default()
            }),
        )
        .unwrap();
    assert_eq!(
        ids.len(),
        90,
        "all variants, no project-membership duplicates"
    );
    let before = serde_json::to_value(
        library
            .query(CatalogQuery {
                limit: 100,
                ..Default::default()
            })
            .unwrap(),
    )
    .unwrap();
    for id in &ids {
        library.analyze_audio(*id).unwrap();
    }
    assert_eq!(
        ids.len(),
        90,
        "frozen list never shrinks as rows become analysed"
    );
    assert!(library
        .prepare_audio_analysis(
            sound_query(),
            Some(AudioCriteria {
                analysis: Some("notAnalysed".into()),
                ..Default::default()
            })
        )
        .unwrap()
        .is_empty());
    let wav_ids = library
        .prepare_audio_analysis(
            CatalogQuery {
                format: Some("WAV".into()),
                ..sound_query()
            },
            Some(AudioCriteria {
                channels: Some("mono".into()),
                min_duration_seconds: Some(0.9),
                max_duration_seconds: Some(1.1),
                ..Default::default()
            }),
        )
        .unwrap();
    assert_eq!(wav_ids.len(), 45);
    drop(library);
    let library = Library::open(&root.join("Library")).unwrap();
    let after = library
        .query(CatalogQuery {
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert!(after
        .items
        .iter()
        .flat_map(|a| &a.files)
        .all(|f| f.audio.is_some()));
    let mut after_json = serde_json::to_value(&after).unwrap();
    for item in after_json["items"].as_array_mut().unwrap() {
        for f in item["files"].as_array_mut().unwrap() {
            f["audio"] = serde_json::Value::Null;
        }
    }
    assert_eq!(
        before, after_json,
        "source provenance/annotations/membership unchanged"
    );
    for i in 0..45 {
        assert_eq!(
            fs::read(root.join(format!("source/Nature/Stream{i:03}.wav"))).unwrap(),
            original
        );
        assert_eq!(
            fs::read(root.join(format!("source/Nature/Stream{i:03}.mp3"))).unwrap(),
            include_bytes!("../tests/fixtures/audio/stereo44100.mp3")
        );
    }
    assert!(
        library
            .prepare_audio_analysis(CatalogQuery::default(), None)
            .is_err(),
        "explicit Sound scope required"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completed_audio_measurements_require_the_same_active_source_and_live_commit_owner() {
    use std::sync::atomic::Ordering;
    let root = workspace("stable");
    fs::write(root.join("source/Nature/Tone.wav"), wav()).unwrap();
    let mut library = Library::create(&root.join("Library")).unwrap();
    library.import_folder(&root.join("source")).unwrap();
    let id = library.prepare_audio_analysis(sound_query(), None).unwrap()[0];
    let cancel = AtomicBool::new(false);
    let good = library.inspect_audio(id, &cancel).unwrap();
    library.store_audio_inspection(id, good, &cancel).unwrap();
    let prior = serde_json::to_value(library.query(sound_query()).unwrap()).unwrap();
    let measured = library.inspect_audio(id, &cancel).unwrap();
    cancel.store(true, Ordering::Relaxed);
    assert!(library
        .store_audio_inspection(id, measured, &cancel)
        .is_err());
    assert_eq!(
        prior,
        serde_json::to_value(library.query(sound_query()).unwrap()).unwrap()
    );
    cancel.store(false, Ordering::Relaxed);
    let measured = library.inspect_audio(id, &cancel).unwrap();
    let path = library.resolve_preview(id).unwrap();
    let held = path.with_extension("held");
    fs::rename(&path, &held).unwrap();
    fs::write(&path, wav()).unwrap();
    assert!(library
        .store_audio_inspection(id, measured, &cancel)
        .is_err());
    assert_eq!(
        prior,
        serde_json::to_value(library.query(sound_query()).unwrap()).unwrap()
    );
    #[cfg(unix)]
    {
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&held, &path).unwrap();
        assert!(library.analyze_audio(id).is_err());
        assert!(library.audio_waveform(id, &cancel).is_err());
        fs::remove_file(&path).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(made.success());
        let started = std::time::Instant::now();
        assert!(library.analyze_audio(id).is_err());
        assert!(library.audio_waveform(id, &cancel).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    fs::remove_file(&path).unwrap();
    fs::rename(&held, &path).unwrap();
    let measured = library.inspect_audio(id, &cancel).unwrap();
    library.trash_file(id).unwrap();
    assert!(library
        .store_audio_inspection(id, measured, &cancel)
        .is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn counted_mp3_duration_is_retained_while_bitrate_estimates_remain_unavailable() {
    let root = workspace("mp3-accuracy");
    let exact = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
    let no_count = include_bytes!("../tests/fixtures/audio/stereo44100-no-count.mp3");
    assert!(exact.windows(4).any(|v| v == b"Info"));
    assert!(!no_count
        .windows(4)
        .any(|v| matches!(v, b"Info" | b"Xing" | b"VBRI")));
    fs::write(root.join("source/Nature/Counted.mp3"), exact).unwrap();
    fs::write(root.join("source/Nature/NoCount.mp3"), no_count).unwrap();
    fs::write(root.join("source/Nature/NoCountInWav.wav"), no_count).unwrap();
    let mut library = Library::create(&root.join("Library")).unwrap();
    library.import_folder(&root.join("source")).unwrap();
    for asset in library.query(sound_query()).unwrap().items {
        let observed = library.analyze_audio(asset.files[0].id).unwrap();
        assert_eq!(observed.status, AudioStatus::Ready, "{:?}", observed.error);
        assert_eq!(observed.sample_rate, Some(44100));
        assert_eq!(observed.channels, Some(2));
        if asset.name == "Counted" {
            assert!((observed.duration_seconds.unwrap() - 0.25).abs() < 0.001);
        } else {
            assert_eq!(
                observed.duration_seconds, None,
                "bitrate fallback is not an exact measurement"
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn truncated_wav_declared_duration_is_an_error_and_cannot_match_exact_filters() {
    let root = workspace("truncated-wave");
    let mut truncated = wav();
    truncated.truncate(1000);
    fs::write(root.join("source/Nature/Truncated.wav"), truncated).unwrap();
    let mut library = Library::create(&root.join("Library")).unwrap();
    library.import_folder(&root.join("source")).unwrap();
    let id = library.prepare_audio_analysis(sound_query(), None).unwrap()[0];
    let observed = library.analyze_audio(id).unwrap();
    assert_eq!(observed.status, AudioStatus::Error);
    assert_eq!(observed.duration_seconds, None);
    assert!(library
        .prepare_audio_analysis(
            sound_query(),
            Some(AudioCriteria {
                min_duration_seconds: Some(0.9),
                ..Default::default()
            })
        )
        .unwrap()
        .is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_container_metadata_is_rejected_before_reader_allocation_without_disabling_audio_tags_profiles(
) {
    let root = workspace("containers");
    let original = wav();
    let mut oversized_info = original[..36].to_vec();
    let value = vec![b'A'; 4098];
    let list_size = 4 + 8 + value.len();
    oversized_info.extend_from_slice(b"LIST");
    oversized_info.extend_from_slice(&(list_size as u32).to_le_bytes());
    oversized_info.extend_from_slice(b"INFOINAM");
    oversized_info.extend_from_slice(&(value.len() as u32).to_le_bytes());
    oversized_info.extend_from_slice(&value);
    oversized_info.extend_from_slice(&original[36..]);
    let riff = oversized_info.len() as u32 - 8;
    oversized_info[4..8].copy_from_slice(&riff.to_le_bytes());
    fs::write(root.join("source/Nature/Oversized.wav"), oversized_info).unwrap();
    let mut unsupported_tags = original[..36].to_vec();
    unsupported_tags.extend_from_slice(b"id3 ");
    unsupported_tags.extend_from_slice(&10_u32.to_le_bytes());
    unsupported_tags.extend_from_slice(b"ID3\x02\0\0\0\0\0\0");
    unsupported_tags.extend_from_slice(&original[36..]);
    let riff = unsupported_tags.len() as u32 - 8;
    unsupported_tags[4..8].copy_from_slice(&riff.to_le_bytes());
    fs::write(
        root.join("source/Nature/UnsupportedTags.wav"),
        unsupported_tags,
    )
    .unwrap();
    let flac = include_bytes!("../tests/fixtures/audio/stereo48000.flac");
    let mut bad_comment = flac[..42].to_vec();
    bad_comment[4] &= 0x7f;
    bad_comment.extend_from_slice(&[0x84, 0, 0, 8]);
    bad_comment.extend_from_slice(&0_u32.to_le_bytes());
    bad_comment.extend_from_slice(&u32::MAX.to_le_bytes());
    bad_comment.extend_from_slice(&flac[42..]);
    fs::write(root.join("source/Nature/BadComment.flac"), bad_comment).unwrap();
    let mut bad_picture = flac[..42].to_vec();
    bad_picture[4] &= 0x7f;
    bad_picture.extend_from_slice(&[0x86, 0, 0, 8]);
    bad_picture.extend_from_slice(&3_u32.to_be_bytes());
    bad_picture.extend_from_slice(&u32::MAX.to_be_bytes());
    bad_picture.extend_from_slice(&flac[42..]);
    fs::write(root.join("source/Nature/BadPicture.flac"), bad_picture).unwrap();
    let mut too_many_seeks = flac[..42].to_vec();
    too_many_seeks[4] &= 0x7f;
    let size = 4097 * 18u32;
    let b = size.to_be_bytes();
    too_many_seeks.extend_from_slice(&[0x83, b[1], b[2], b[3]]);
    too_many_seeks.resize(too_many_seeks.len() + size as usize, 0);
    too_many_seeks.extend_from_slice(&flac[42..]);
    fs::write(root.join("source/Nature/TooManySeeks.flac"), too_many_seeks).unwrap();
    let mut library = Library::create(&root.join("Library")).unwrap();
    library.import_folder(&root.join("source")).unwrap();
    for asset in library.query(sound_query()).unwrap().items {
        let observed = library.analyze_audio(asset.files[0].id).unwrap();
        match asset.name.as_str() {
            "UnsupportedTags" => {
                assert_eq!(observed.status, AudioStatus::Ready, "{:?}", observed.error);
                assert_eq!(observed.duration_seconds, Some(1.0));
            }
            "Oversized" => assert!(observed.error.unwrap().contains("WAV-INFO")),
            "BadComment" => assert!(observed.error.unwrap().contains("256 Einträge")),
            "BadPicture" => assert!(observed.error.unwrap().contains("4 KiB")),
            "TooManySeeks" => assert!(observed.error.unwrap().contains("4096 Einträge")),
            _ => panic!("unexpected fixture"),
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_audio_variants_include_managed_zip_and_external_sources_with_reanalysis_and_unavailable_rows_preserved(
) {
    use std::io::Write;
    let root = workspace("source-modes");
    fs::write(root.join("source/Nature/Tone.wav"), wav()).unwrap();
    fs::write(root.join("source/Nature/Broken.wav"), b"not an audio file").unwrap();
    let archive = root.join("Zip.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    zip.start_file(
        "Original/Tone.ogg",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(include_bytes!("../tests/fixtures/audio/stereo32000.ogg"))
        .unwrap();
    zip.finish().unwrap();
    fs::create_dir(root.join("External")).unwrap();
    fs::write(
        root.join("External/Tone.flac"),
        include_bytes!("../tests/fixtures/audio/stereo48000.flac"),
    )
    .unwrap();
    let mut library = Library::create(&root.join("Library")).unwrap();
    let folder = library.import_folder(&root.join("source")).unwrap();
    let packaged = library.import_zip(&archive).unwrap();
    let external = library.reference_folder(&root.join("External")).unwrap();
    for package in [folder.id, packaged.id, external.id] {
        let ids = library
            .prepare_audio_analysis(
                CatalogQuery {
                    package_id: Some(package),
                    ..sound_query()
                },
                None,
            )
            .unwrap();
        for id in ids {
            library.analyze_audio(id).unwrap();
        }
    }
    let before = library.query(sound_query()).unwrap();
    assert_eq!(before.total, 4);
    let errors = library
        .prepare_audio_analysis(
            sound_query(),
            Some(AudioCriteria {
                analysis: Some("error".into()),
                ..Default::default()
            }),
        )
        .unwrap();
    assert_eq!(errors.len(), 1);
    let ready = library
        .prepare_audio_analysis(
            sound_query(),
            Some(AudioCriteria {
                analysis: Some("ready".into()),
                ..Default::default()
            }),
        )
        .unwrap();
    assert_eq!(ready.len(), 3);
    for id in &ready {
        library.analyze_audio(*id).unwrap();
    }
    let prior = serde_json::to_value(library.query(sound_query()).unwrap()).unwrap();
    fs::rename(root.join("External"), root.join("External-offline")).unwrap();
    let external_id = library
        .prepare_audio_analysis(
            CatalogQuery {
                package_id: Some(external.id),
                ..sound_query()
            },
            None,
        )
        .unwrap()[0];
    assert!(library.analyze_audio(external_id).is_err());
    let offline = library.query(sound_query()).unwrap();
    let before_audio = prior["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["packageId"] == external.id)
        .unwrap()["files"][0]["audio"]
        .clone();
    let after_file = &offline
        .items
        .iter()
        .find(|a| a.package_id == external.id)
        .unwrap()
        .files[0];
    assert!(!after_file.available);
    assert_eq!(
        before_audio,
        serde_json::to_value(&after_file.audio).unwrap()
    );
    fs::rename(root.join("External-offline"), root.join("External")).unwrap();
    drop(library);
    let library = Library::open(&root.join("Library")).unwrap();
    assert_eq!(
        prior,
        serde_json::to_value(library.query(sound_query()).unwrap()).unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn safe_audio_waveform_reads_observe_live_cancellation_without_retrying_interrupted_forever() {
    use std::sync::{atomic::Ordering, Arc};
    let root = workspace("live-cancel");
    let path = root.join("source/Nature/Long.wav");
    let mut header = wav()[..44].to_vec();
    let data_size = 80_000_000_u32;
    header[4..8].copy_from_slice(&(data_size + 36).to_le_bytes());
    header[40..44].copy_from_slice(&data_size.to_le_bytes());
    use std::io::Write;
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(&header).unwrap();
    file.set_len(u64::from(data_size) + 44).unwrap();
    drop(file);
    let mut library = Library::create(&root.join("Library")).unwrap();
    library.reference_folder(&root.join("source")).unwrap();
    let id = library.prepare_audio_analysis(sound_query(), None).unwrap()[0];
    let flag = Arc::new(AtomicBool::new(false));
    let other = flag.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(3));
        other.store(true, Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    assert!(library.audio_waveform(id, &flag).is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    thread.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn ogg_page(flags: u8, serial: u32, sequence: u32, lacing: &[u8], body: &[u8]) -> Vec<u8> {
    use symphonia::core::{checksum::Crc32, io::Monitor};
    let mut page = b"OggS".to_vec();
    page.extend_from_slice(&[0, flags]);
    page.extend_from_slice(&0_u64.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&0_u32.to_le_bytes());
    page.push(lacing.len() as u8);
    page.extend_from_slice(lacing);
    page.extend_from_slice(body);
    let mut crc = Crc32::new(0);
    crc.process_buf_bytes(&page);
    page[22..26].copy_from_slice(&crc.crc().to_le_bytes());
    page
}
#[test]
fn ogg_late_metadata_and_continued_audio_packets_cannot_bypass_preflight_limits() {
    let root = workspace("ogg-late");
    let original = include_bytes!("../tests/fixtures/audio/stereo32000.ogg");
    let mut offset = 0;
    for _ in 0..2 {
        let segments = original[offset + 26] as usize;
        let body: usize = original[offset + 27..offset + 27 + segments]
            .iter()
            .map(|v| *v as usize)
            .sum();
        offset += 27 + segments + body;
    }
    let headers = &original[..offset];
    let serial = u32::from_le_bytes(original[14..18].try_into().unwrap());
    let mut late = headers.to_vec();
    let mut comment = b"\x03vorbis".to_vec();
    comment.extend_from_slice(&0_u32.to_le_bytes());
    comment.extend_from_slice(&u32::MAX.to_le_bytes());
    comment.push(1);
    late.extend_from_slice(&ogg_page(4, serial, 2, &[comment.len() as u8], &comment));
    fs::write(root.join("source/Nature/LateComment.ogg"), late).unwrap();
    let mut continued = headers.to_vec();
    let body = vec![0; 255 * 255];
    for i in 0..18 {
        continued.extend_from_slice(&ogg_page(
            if i == 0 { 0 } else { 1 },
            serial,
            2 + i,
            &[255; 255],
            &body,
        ));
    }
    continued.extend_from_slice(&ogg_page(5, serial, 20, &[0], &[]));
    fs::write(root.join("source/Nature/Continued.ogg"), continued).unwrap();
    let mut library = Library::create(&root.join("Library")).unwrap();
    library.import_folder(&root.join("source")).unwrap();
    let started = std::time::Instant::now();
    for asset in library.query(sound_query()).unwrap().items {
        let observed = library.analyze_audio(asset.files[0].id).unwrap();
        assert_eq!(observed.status, AudioStatus::Error);
        if asset.name == "LateComment" {
            assert!(observed.error.unwrap().contains("weiteren Vorbis-Header"));
        } else {
            assert!(observed.error.unwrap().contains("1 MiB"));
        }
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn waiting_audio_commits_revalidate_cancellation_and_source_after_sqlite_writer_lock() {
    use std::sync::{atomic::Ordering, Arc};
    let root = workspace("commit-wait");
    fs::write(root.join("source/Nature/Tone.wav"), wav()).unwrap();
    let mut library = Library::create(&root.join("Library")).unwrap();
    library.import_folder(&root.join("source")).unwrap();
    let id = library.prepare_audio_analysis(sound_query(), None).unwrap()[0];
    library.analyze_audio(id).unwrap();
    library.audio_analysis_busy_timeout().unwrap();
    let prior = serde_json::to_value(library.query(sound_query()).unwrap()).unwrap();
    let measured = library.inspect_audio(id, &AtomicBool::new(false)).unwrap();
    let blocker = Connection::open(root.join("Library/catalog.sqlite3")).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let other = flag.clone();
    let wait = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(15));
        other.store(true, Ordering::Relaxed);
        blocker.execute_batch("COMMIT").unwrap();
    });
    assert!(
        library.store_audio_inspection(id, measured, &flag).is_err(),
        "cancel during writer wait must rollback"
    );
    wait.join().unwrap();
    assert_eq!(
        prior,
        serde_json::to_value(library.query(sound_query()).unwrap()).unwrap()
    );
    let flag = AtomicBool::new(false);
    let measured = library.inspect_audio(id, &flag).unwrap();
    let path = library.resolve_preview(id).unwrap();
    let changed = path.clone();
    let blocker = Connection::open(root.join("Library/catalog.sqlite3")).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let wait = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(15));
        fs::write(changed, b"changed during writer wait").unwrap();
        blocker.execute_batch("COMMIT").unwrap();
    });
    assert!(
        library.store_audio_inspection(id, measured, &flag).is_err(),
        "source changed during writer wait must preserve prior row"
    );
    wait.join().unwrap();
    assert_eq!(
        prior,
        serde_json::to_value(library.query(sound_query()).unwrap()).unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oversized_audio_scope_rejects_instead_of_silently_truncating_variant_count() {
    let root = workspace("queue-limit");
    fs::write(root.join("source/Nature/Tone.wav"), wav()).unwrap();
    let mut library = Library::create(&root.join("Library")).unwrap();
    library.import_folder(&root.join("source")).unwrap();
    // A catalogue-only boundary fixture; it is not a claim of 100,001 physical-file performance.
    library.connection.execute_batch("WITH RECURSIVE n(value) AS (SELECT 2 UNION ALL SELECT value + 1 FROM n WHERE value < 100001) INSERT INTO asset_files(asset_id, original_filename, relative_path, format, size_bytes) SELECT 1, 'Variant' || value || '.wav', 'packages/variant' || value || '.wav', 'WAV', 1 FROM n").unwrap();
    let error = library
        .prepare_audio_analysis(sound_query(), None)
        .unwrap_err();
    assert!(error.contains("100.000"));
    fs::remove_dir_all(root).unwrap();
}
