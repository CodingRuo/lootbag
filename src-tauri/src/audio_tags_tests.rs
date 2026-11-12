use super::*;
use crate::library::{AudioStatus, CatalogQuery, Library};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "game-assets-audio-tags-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn embedded_audio_tags_retains_real_wav_values_and_catalogue_annotations_on_reopen() {
    let workspace = Workspace::new();
    let source = workspace.0.join("Original Package");
    fs::create_dir(&source).unwrap();
    let bytes = include_bytes!("../tests/fixtures/tagged-audio.wav");
    fs::write(source.join("A001.wav"), bytes).unwrap();
    let root = workspace.0.join("Library");
    let mut library = Library::create(&root).unwrap();
    let package = library.import_folder(&source).unwrap();
    let asset = &library.query(CatalogQuery::default()).unwrap().items[0];
    let asset_id = asset.id;
    let file_id = asset.files[0].id;
    library
        .annotate_asset(asset_id, "My river", &["manual ambience".into()])
        .unwrap();
    library
        .annotate_package(package.id, &["my project".into()])
        .unwrap();
    drop(library);
    let library = Library::open_read_only(&root).unwrap();
    let tags = library
        .embedded_audio_tags(file_id, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(tags.title.as_deref(), Some("Salzburg River"));
    assert_eq!(tags.artist.as_deref(), Some("Owned Fixture"));
    assert_eq!(tags.album.as_deref(), Some("Fixture Collection"));
    assert_eq!(tags.genre.as_deref(), Some("Ambient"));
    let asset = &library.query(CatalogQuery::default()).unwrap().items[0];
    assert_eq!(asset.id, asset_id);
    assert_eq!(asset.name, "My river");
    assert_eq!(asset.tags, ["manual ambience"]);
    assert_eq!(asset.package_name, "Original Package");
    assert_eq!(asset.files[0].original_filename, "A001.wav");
    assert_eq!(library.packages().unwrap()[0].tags, ["my project"]);
    assert_eq!(
        fs::read(library.resolve_preview(file_id).unwrap()).unwrap(),
        bytes
    );
    assert_eq!(fs::read(source.join("A001.wav")).unwrap(), bytes);
}

#[test]
fn embedded_audio_tags_reads_all_four_real_variants_from_managed_zip_and_referenced_packages() {
    use std::io::Write;
    let workspace = Workspace::new();
    let source = workspace.0.join("Tagged Package");
    fs::create_dir_all(source.join("Audio")).unwrap();
    let fixtures: &[(&str, &[u8])] = &[
        (
            "A001.wav",
            include_bytes!("../tests/fixtures/tagged-audio.wav"),
        ),
        (
            "A001.mp3",
            include_bytes!("../tests/fixtures/tagged-audio.mp3"),
        ),
        (
            "A001.ogg",
            include_bytes!("../tests/fixtures/tagged-audio.ogg"),
        ),
        (
            "A001.flac",
            include_bytes!("../tests/fixtures/tagged-audio.flac"),
        ),
    ];
    for (name, bytes) in fixtures {
        fs::write(source.join("Audio").join(name), bytes).unwrap();
    }
    let archive = workspace.0.join("Tagged Package.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for (name, bytes) in fixtures {
        zip.start_file(
            format!("Audio/{name}"),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    for mode in ["managed", "zip", "reference"] {
        let root = workspace.0.join(mode);
        let mut library = Library::create(&root).unwrap();
        let package = match mode {
            "managed" => library.import_folder(&source).unwrap(),
            "zip" => library.import_zip(&archive).unwrap(),
            _ => library.reference_folder(&source).unwrap(),
        };
        let asset = &library.query(CatalogQuery::default()).unwrap().items[0];
        library
            .annotate_asset(asset.id, "My chosen sound", &["chosen manually".into()])
            .unwrap();
        library
            .annotate_package(package.id, &["project selection".into()])
            .unwrap();
        let before =
            serde_json::to_value(library.query(CatalogQuery::default()).unwrap().items).unwrap();
        drop(library);
        let library = Library::open_read_only(&root).unwrap();
        let asset = &library.query(CatalogQuery::default()).unwrap().items[0];
        assert_eq!(asset.files.len(), 4);
        for file in &asset.files {
            let tags = library
                .embedded_audio_tags(file.id, &AtomicBool::new(false))
                .unwrap();
            assert_eq!(
                tags.title.as_deref(),
                Some("Salzburg River"),
                "{mode} {}",
                file.format
            );
            assert_eq!(tags.artist.as_deref(), Some("Owned Fixture"));
            assert_eq!(tags.album.as_deref(), Some("Fixture Collection"));
            assert_eq!(tags.genre.as_deref(), Some("Ambient"));
            assert_eq!(tags.status, AudioTagStatus::Ready);
            assert_eq!(file.original_folder, "Audio");
            let expected = fixtures
                .iter()
                .find(|(name, _)| *name == file.original_filename)
                .unwrap()
                .1;
            assert_eq!(
                fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
                expected
            );
            assert_eq!(
                fs::read(source.join("Audio").join(&file.original_filename)).unwrap(),
                expected
            );
        }
        assert_eq!(
            serde_json::to_value(library.query(CatalogQuery::default()).unwrap().items).unwrap(),
            before
        );
    }
}

#[cfg(unix)]
#[test]
fn embedded_audio_tags_open_rejects_a_substituted_symlink_without_reading_or_changing_target() {
    let workspace = Workspace::new();
    let original = workspace.0.join("original.wav");
    let replacement = workspace.0.join("replaced.wav");
    let bytes = include_bytes!("../tests/fixtures/tagged-audio.wav");
    fs::write(&original, bytes).unwrap();
    std::os::unix::fs::symlink(&original, &replacement).unwrap();
    assert!(open_source(&replacement).is_err());
    assert!(fs::symlink_metadata(&replacement)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&original).unwrap(), bytes);
}

#[test]
fn embedded_audio_tags_cancellation_during_io_returns_promptly_and_reading_can_recover() {
    use std::io::Read;
    let workspace = Workspace::new();
    let path = workspace.0.join("sound.wav");
    fs::write(&path, include_bytes!("../tests/fixtures/tagged-audio.wav")).unwrap();
    let cancelled = AtomicBool::new(false);
    let mut source = TagSource {
        file: fs::File::open(&path).unwrap(),
        read: 0,
        cancelled: &cancelled,
        started: Instant::now(),
    };
    let mut first = [0; 12];
    source.read_exact(&mut first).unwrap();
    cancelled.store(true, Ordering::Relaxed);
    let start = Instant::now();
    let error = source.read_exact(&mut first).unwrap_err();
    assert!(error.to_string().contains("abgebrochen"), "{error}");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "cancellation must not busy-retry Interrupted until deadline"
    );
    cancelled.store(false, Ordering::Relaxed);
    source.read_exact(&mut first).unwrap();
}

#[cfg(unix)]
#[test]
fn embedded_audio_tags_open_rejects_a_substituted_fifo_promptly_and_preserves_it() {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let workspace = Workspace::new();
    let fifo = workspace.0.join("replaced.wav");
    assert!(std::process::Command::new("/usr/bin/mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let before = fs::symlink_metadata(&fifo).unwrap();
    let start = Instant::now();
    assert!(open_source(&fifo).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
    let after = fs::symlink_metadata(&fifo).unwrap();
    assert!(after.file_type().is_fifo());
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
}

fn read_owned_bytes(name: &str, bytes: &[u8]) -> Result<EmbeddedAudioTags> {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(name), bytes).unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    let file = &library.query(CatalogQuery::default()).unwrap().items[0].files[0];
    let result = library.embedded_audio_tags(file.id, &AtomicBool::new(false));
    assert_eq!(fs::read(source.join(name)).unwrap(), bytes);
    assert_eq!(
        fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
        bytes
    );
    result
}

#[test]
fn embedded_audio_tags_invalid_wav_text_is_visible_instead_of_replacement_characters() {
    let mut bytes = include_bytes!("../tests/fixtures/tagged-audio.wav").to_vec();
    bytes[94] = 0xff; // Independent fixture layout: INAM payload starts at byte 94.
    let error = read_owned_bytes("bad.wav", &bytes).unwrap_err();
    assert!(
        error.contains("UTF-8") || error.contains("Zeichenkodierung"),
        "{error}"
    );
    assert_eq!(
        read_owned_bytes(
            "valid.wav",
            include_bytes!("../tests/fixtures/tagged-audio.wav")
        )
        .unwrap()
        .title
        .as_deref(),
        Some("Salzburg River")
    );
}

// Authored container envelopes exercise text guards; the four files above qualify real codecs.
fn id3_file(version: u8, frames: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    fn size(value: usize) -> [u8; 4] {
        [
            ((value >> 21) & 127) as u8,
            ((value >> 14) & 127) as u8,
            ((value >> 7) & 127) as u8,
            (value & 127) as u8,
        ]
    }
    let mut body = Vec::new();
    for (name, value) in frames {
        body.extend_from_slice(*name);
        body.extend_from_slice(&if version == 4 {
            size(value.len())
        } else {
            (value.len() as u32).to_be_bytes()
        });
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(value);
    }
    let mut bytes = b"ID3".to_vec();
    bytes.extend_from_slice(&[version, 0, 0]);
    bytes.extend_from_slice(&size(body.len()));
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(&include_bytes!("../tests/fixtures/tagged-audio.mp3")[144..]);
    bytes
}

#[test]
fn embedded_audio_tags_rejects_oversized_unknown_id3_frames_before_the_reader() {
    let bytes = id3_file(
        4,
        &[(b"TIT2", b"\x03Known title"), (b"TXXX", &vec![b'x'; 4097])],
    );
    let error = read_owned_bytes("large.mp3", &bytes).unwrap_err();
    assert!(error.contains("4 KiB"), "{error}");
}

fn riff_chunk(name: &[u8; 4], value: &[u8]) -> Vec<u8> {
    let mut bytes = name.to_vec();
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value);
    if value.len() & 1 != 0 {
        bytes.push(0);
    }
    bytes
}
fn riff_file(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
    for chunk in chunks {
        bytes.extend_from_slice(chunk);
    }
    let size = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    bytes
}
fn info(fields: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut bytes = b"INFO".to_vec();
    for (name, value) in fields {
        bytes.extend_from_slice(&riff_chunk(name, value));
    }
    riff_chunk(b"LIST", &bytes)
}
fn comments(entries: &[&[u8]]) -> Vec<u8> {
    let mut bytes = 7u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(b"Fixture");
    bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        bytes.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        bytes.extend_from_slice(entry);
    }
    bytes
}
fn flac_file(blocks: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend_from_slice(&[0, 0, 0, 34]);
    bytes.extend_from_slice(&include_bytes!("../tests/fixtures/tagged-audio.flac")[8..42]);
    for (index, (kind, value)) in blocks.iter().enumerate() {
        bytes.push(kind | if index + 1 == blocks.len() { 128 } else { 0 });
        let size = (value.len() as u32).to_be_bytes();
        bytes.extend_from_slice(&size[1..]);
        bytes.extend_from_slice(value);
    }
    bytes
}
fn ogg_page(sequence: u32, flags: u8, serial: u32, lacing: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut bytes = b"OggS\0".to_vec();
    bytes.push(flags);
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&serial.to_le_bytes());
    bytes.extend_from_slice(&sequence.to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.push(lacing.len() as u8);
    bytes.extend_from_slice(lacing);
    bytes.extend_from_slice(payload);
    let mut crc = Crc32::new(0);
    crc.process_buf_bytes(&bytes);
    bytes[22..26].copy_from_slice(&crc.crc().to_le_bytes());
    bytes
}
fn ogg_file(comment: &[u8]) -> Vec<u8> {
    let mut bytes = ogg_page(
        0,
        2,
        42,
        &[30],
        &include_bytes!("../tests/fixtures/tagged-audio.ogg")[28..58],
    );
    let mut packet = b"\x03vorbis".to_vec();
    packet.extend_from_slice(comment);
    packet.push(1);
    let mut lacing = vec![255; packet.len() / 255];
    lacing.push((packet.len() % 255) as u8);
    bytes.extend_from_slice(&ogg_page(1, 0, 42, &lacing, &packet));
    bytes
}

#[test]
fn embedded_audio_tags_absent_fields_are_explicit_without_filename_inference() {
    let wav = riff_file(&[
        include_bytes!("../tests/fixtures/tagged-audio.wav")[12..36].to_vec(),
        include_bytes!("../tests/fixtures/tagged-audio.wav")[160..].to_vec(),
    ]);
    let mp3 = &include_bytes!("../tests/fixtures/tagged-audio.mp3")[144..];
    let flac = flac_file(&[(4, comments(&[b"ENCODER=Fixture"]))]);
    let ogg = ogg_file(&comments(&[b"ENCODER=Fixture"]));
    for (name, bytes) in [
        ("Salzburg.wav", wav.as_slice()),
        ("Artist.mp3", mp3),
        ("Album.flac", flac.as_slice()),
        ("Genre.ogg", ogg.as_slice()),
    ] {
        let tags = read_owned_bytes(name, bytes).unwrap();
        assert_eq!(tags.status, AudioTagStatus::Ready);
        assert_eq!(
            (tags.title, tags.artist, tags.album, tags.genre),
            (None, None, None, None)
        );
        assert!(tags.message.is_none());
    }
}

#[test]
fn embedded_audio_tags_reads_info_after_audio_and_preserves_odd_padding_and_source_text() {
    let wav = riff_file(&[
        include_bytes!("../tests/fixtures/tagged-audio.wav")[12..36].to_vec(),
        riff_chunk(b"data", &vec![0; 3 * 1024 * 1024]),
        riff_chunk(b"JUNK", b"odd"),
        info(&[(b"INAM", b"  River  \0\0"), (b"IART", "René".as_bytes())]),
    ]);
    let tags = read_owned_bytes("after-data.wav", &wav).unwrap();
    assert_eq!(tags.title.as_deref(), Some("  River  "));
    assert_eq!(tags.artist.as_deref(), Some("René"));
    assert_eq!(tags.album, None);
}

#[test]
fn wav_with_final_odd_chunk_without_pad_byte_has_duration_and_tags() {
    // 24-bit mono, 44101 frames: the final data chunk is odd-sized and the file ends without its pad byte.
    let mut format = 1_u16.to_le_bytes().to_vec();
    format.extend_from_slice(&1_u16.to_le_bytes());
    format.extend_from_slice(&44_100_u32.to_le_bytes());
    format.extend_from_slice(&(44_100_u32 * 3).to_le_bytes());
    format.extend_from_slice(&3_u16.to_le_bytes());
    format.extend_from_slice(&24_u16.to_le_bytes());
    let mut data = riff_chunk(b"data", &vec![0; 44_101 * 3]);
    data.pop();
    let wav = riff_file(&[
        riff_chunk(b"fmt ", &format),
        info(&[(b"INAM", b"River")]),
        data,
    ]);
    assert_eq!(wav.len() % 2, 1);
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("odd.wav"), &wav).unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    let file = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    let tags = library
        .embedded_audio_tags(file, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(tags.title.as_deref(), Some("River"));
    let metadata = library.analyze_audio(file).unwrap();
    assert_eq!(metadata.status, AudioStatus::Ready, "{:?}", metadata.error);
    assert!((metadata.duration_seconds.unwrap() - 44_101.0 / 44_100.0).abs() < 1e-6);
}

#[test]
fn embedded_audio_tags_limits_count_and_bytes_across_all_riff_info_lists() {
    let many: Vec<(&[u8; 4], &[u8])> = (0..129).map(|_| (b"IUNK", b"x".as_slice())).collect();
    let unknown = vec![b'a'; 4096];
    let cases = [
        (riff_file(&[info(&[(b"INAM", &vec![b'a'; 1025])])]), "1 KiB"),
        (
            riff_file(&[info(&[(b"IUNK", &vec![b'a'; 4097])])]),
            "Grenzen",
        ),
        (riff_file(&[info(&many), info(&many)]), "Grenzen"),
        (
            riff_file(&[info(&vec![(b"IUNK", unknown.as_slice()); 65])]),
            "Grenzen",
        ),
    ];
    for (bytes, message) in cases {
        let error = read_owned_bytes("large.wav", &bytes).unwrap_err();
        assert!(error.contains(message), "{error}");
    }
    let mut truncated = include_bytes!("../tests/fixtures/tagged-audio.wav").to_vec();
    truncated.pop();
    assert!(read_owned_bytes("truncated.wav", &truncated)
        .unwrap_err()
        .contains("unvollständig"));
    let mut outside = riff_file(&[riff_chunk(b"data", b"x")]);
    outside[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(read_owned_bytes("outside.wav", &outside)
        .unwrap_err()
        .contains("Container"));
}

#[test]
fn embedded_audio_tags_id3_text_envelopes_are_validated_before_partial_values_can_escape() {
    for payload in [
        b"\x04ignored".as_slice(),
        b"\x03\xff",
        b"\x01no-bom",
        b"\x02\xd8\x00",
        b"\x02\x00",
    ] {
        let bytes = id3_file(4, &[(b"TPE1", b"\x03Visible artist"), (b"TIT2", payload)]);
        assert!(read_owned_bytes("malformed.mp3", &bytes).is_err());
    }
    for version in [3, 4] {
        let bytes = id3_file(
            version,
            &[(b"TIT2", b"\x01\xff\xfeR\x00e\x00n\x00\xe9\x00")],
        );
        assert_eq!(
            read_owned_bytes("unicode.mp3", &bytes)
                .unwrap()
                .title
                .as_deref(),
            Some("René")
        );
    }
    let mut padding = id3_file(4, &[]);
    padding.splice(10..10, [0, 1]);
    padding[9] = 2;
    assert!(read_owned_bytes("padding.mp3", &padding)
        .unwrap_err()
        .contains("Padding"));
    let mut size = id3_file(4, &[(b"TIT2", b"\x03title")]);
    size[6] = 0x80;
    assert!(read_owned_bytes("size.mp3", &size)
        .unwrap_err()
        .contains("Größenangabe"));
    let mut frame = id3_file(4, &[(b"TIT2", b"\x03title")]);
    frame[14..18].copy_from_slice(&[0, 0, 2, 0]);
    assert!(read_owned_bytes("frame.mp3", &frame)
        .unwrap_err()
        .contains("unvollständig"));
    let frames = vec![(b"TXXX", b"\x03unknown".as_slice()); 257];
    assert!(read_owned_bytes("count.mp3", &id3_file(4, &frames))
        .unwrap_err()
        .contains("256 Einträge"));
    let mut huge = id3_file(4, &[]);
    huge[6..10].copy_from_slice(&[0, 16, 0, 1]); // 262145 bytes declared, rejected before allocation.
    assert!(read_owned_bytes("aggregate.mp3", &huge)
        .unwrap_err()
        .contains("256 KiB"));
}

#[test]
fn embedded_audio_tags_unsupported_forms_remain_visible_without_partial_supported_fields() {
    let mut rf64 = include_bytes!("../tests/fixtures/tagged-audio.wav").to_vec();
    rf64[..4].copy_from_slice(b"RF64");
    let mut flagged = id3_file(4, &[(b"TIT2", b"\x03Known title")]);
    flagged[5] = 0x80;
    let chapters = id3_file(4, &[(b"TIT2", b"\x03Known title"), (b"CHAP", b"ignored")]);
    let non_vorbis = include_bytes!("../tests/fixtures/audio/unsupported-ogg-flac.ogg").to_vec();
    let mut multiplexed = ogg_file(&comments(&[b"TITLE=Known title"]));
    multiplexed[58 + 14..58 + 18].copy_from_slice(&43u32.to_le_bytes());
    for (name, bytes) in [
        ("rf64.wav", rf64),
        ("flags.mp3", flagged),
        ("chapters.mp3", chapters),
        ("non-vorbis.ogg", non_vorbis),
        ("multiplexed.ogg", multiplexed),
    ] {
        let tags = read_owned_bytes(name, &bytes).unwrap();
        assert_eq!(tags.status, AudioTagStatus::Unsupported, "{name}");
        assert!(tags.message.as_deref().is_some_and(|text| !text.is_empty()));
        assert_eq!(
            (tags.title, tags.artist, tags.album, tags.genre),
            (None, None, None, None)
        );
    }
    let wav = riff_file(&[
        info(&[(b"INAM", b"Known title")]),
        riff_chunk(b"id3 ", b"unqualified"),
    ]);
    let tags = read_owned_bytes("id3.wav", &wav).unwrap();
    assert_eq!(tags.status, AudioTagStatus::Unsupported);
    assert_eq!(tags.title.as_deref(), Some("Known title"));
    assert!(tags.message.unwrap().contains("ID3"));
}

#[test]
fn embedded_audio_tags_flac_skips_large_pictures_and_bounds_all_comment_blocks() {
    let bytes = flac_file(&[
        (6, vec![0; 3 * 1024 * 1024]),
        (2, b"application".to_vec()),
        (4, comments(&[b"TITLE=Salzburg River"])),
    ]);
    assert_eq!(
        read_owned_bytes("picture.flac", &bytes)
            .unwrap()
            .title
            .as_deref(),
        Some("Salzburg River")
    );
    let mut entry = b"IGNORED=".to_vec();
    entry.extend_from_slice(&vec![b'x'; 4088]);
    let block = comments(&[&entry]);
    let blocks = vec![(4, block); 65];
    assert!(read_owned_bytes("aggregate.flac", &flac_file(&blocks))
        .unwrap_err()
        .contains("256 KiB"));
    let mut title = b"TITLE=".to_vec();
    title.extend_from_slice(&vec![b'x'; 1025]);
    assert!(
        read_owned_bytes("text.flac", &flac_file(&[(4, comments(&[&title]))]))
            .unwrap_err()
            .contains("1 KiB")
    );
    let mut truncated = flac_file(&[(4, comments(&[b"TITLE=Known title"]))]);
    truncated.pop();
    assert!(read_owned_bytes("truncated.flac", &truncated)
        .unwrap_err()
        .contains("unvollständig"));
}

#[test]
fn embedded_audio_tags_ogg_continuation_crc_and_framing_are_qualified() {
    let mut packet = b"\x03vorbis".to_vec();
    let mut unknown = b"X=".to_vec();
    unknown.extend_from_slice(&vec![b'x'; 260]);
    packet.extend_from_slice(&comments(&[b"TITLE=Salzburg River", &unknown]));
    packet.push(1);
    let mut bytes = ogg_page(
        0,
        2,
        42,
        &[30],
        &include_bytes!("../tests/fixtures/tagged-audio.ogg")[28..58],
    );
    bytes.extend_from_slice(&ogg_page(1, 0, 42, &[255], &packet[..255]));
    bytes.extend_from_slice(&ogg_page(
        2,
        1,
        42,
        &[(packet.len() - 255) as u8],
        &packet[255..],
    ));
    assert_eq!(
        read_owned_bytes("continued.ogg", &bytes)
            .unwrap()
            .title
            .as_deref(),
        Some("Salzburg River")
    );
    let mut crc = ogg_file(&comments(&[b"TITLE=Known title"]));
    *crc.last_mut().unwrap() ^= 1;
    assert!(read_owned_bytes("crc.ogg", &crc)
        .unwrap_err()
        .contains("Prüfsumme"));
    let mut malformed_packet = b"\x03vorbis".to_vec();
    malformed_packet.extend_from_slice(&comments(&[b"TITLE=Known title"]));
    malformed_packet.push(0);
    let mut framing = ogg_page(
        0,
        2,
        42,
        &[30],
        &include_bytes!("../tests/fixtures/tagged-audio.ogg")[28..58],
    );
    framing.extend_from_slice(&ogg_page(
        1,
        0,
        42,
        &[malformed_packet.len() as u8],
        &malformed_packet,
    ));
    assert!(read_owned_bytes("framing.ogg", &framing)
        .unwrap_err()
        .contains("Tag-Paket"));
    let mut discontinuity = bytes.clone();
    let second = 58 + 27 + 1 + 255;
    discontinuity[second + 5] = 0;
    assert!(read_owned_bytes("continuation.ogg", &discontinuity)
        .unwrap_err()
        .contains("Sequenz"));
    let mut incomplete = ogg_file(&comments(&[b"TITLE=Known title"]));
    incomplete.truncate(incomplete.len() - 4);
    assert!(read_owned_bytes("incomplete.ogg", &incomplete).is_err());
}

#[test]
fn embedded_audio_tags_deadline_and_io_budget_stop_reads_and_seeks() {
    use std::io::{Read, Seek};
    let workspace = Workspace::new();
    let path = workspace.0.join("sound.wav");
    fs::write(&path, include_bytes!("../tests/fixtures/tagged-audio.wav")).unwrap();
    let cancelled = AtomicBool::new(false);
    let mut source = TagSource {
        file: open_source(&path).unwrap(),
        read: 2 * 1024 * 1024,
        cancelled: &cancelled,
        started: Instant::now(),
    };
    assert!(source
        .read_exact(&mut [0; 1])
        .unwrap_err()
        .to_string()
        .contains("2 MiB"));
    source.read = 0;
    source.started = Instant::now() - Duration::from_secs(4);
    assert!(source
        .read_exact(&mut [0; 1])
        .unwrap_err()
        .to_string()
        .contains("3 Sekunden"));
    assert!(source.seek(SeekFrom::Start(0)).is_err());
}

#[test]
fn embedded_audio_tags_rejects_cancelled_missing_changed_inactive_and_non_sound_sources() {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    let original = include_bytes!("../tests/fixtures/tagged-audio.wav");
    fs::write(source.join("sound.wav"), original).unwrap();
    fs::write(
        source.join("image.png"),
        include_bytes!("../tests/fixtures/sprite-atlas.png"),
    )
    .unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.reference_folder(&source).unwrap();
    let items = library.query(CatalogQuery::default()).unwrap().items;
    let sound = items
        .iter()
        .find(|asset| asset.files[0].format == "WAV")
        .unwrap()
        .files[0]
        .id;
    let image = items
        .iter()
        .find(|asset| asset.files[0].format == "PNG")
        .unwrap()
        .files[0]
        .id;
    assert!(library
        .embedded_audio_tags(sound, &AtomicBool::new(true))
        .unwrap_err()
        .contains("abgebrochen"));
    assert!(library
        .embedded_audio_tags(image, &AtomicBool::new(false))
        .unwrap_err()
        .contains("kein Sound"));
    assert!(library
        .embedded_audio_tags(-1, &AtomicBool::new(false))
        .is_err());
    library.trash_file(sound).unwrap();
    assert!(library
        .embedded_audio_tags(sound, &AtomicBool::new(false))
        .is_err());
    let trash = library.trash_entries().unwrap();
    library.restore_trash(trash[0].id).unwrap();
    assert_eq!(
        library
            .embedded_audio_tags(sound, &AtomicBool::new(false))
            .unwrap()
            .title
            .as_deref(),
        Some("Salzburg River")
    );
    let renamed = source.join("old.wav");
    fs::rename(source.join("sound.wav"), &renamed).unwrap();
    assert!(library
        .embedded_audio_tags(sound, &AtomicBool::new(false))
        .is_err());
    fs::write(source.join("sound.wav"), original).unwrap();
    assert!(library
        .embedded_audio_tags(sound, &AtomicBool::new(false))
        .is_err());
    assert_eq!(fs::read(&renamed).unwrap(), original);
    assert_eq!(fs::read(source.join("sound.wav")).unwrap(), original);
    library.trash_file(sound).unwrap();
    assert!(library
        .embedded_audio_tags(sound, &AtomicBool::new(false))
        .is_err());
    assert_eq!(fs::read(source.join("sound.wav")).unwrap(), original);
}

#[test]
fn embedded_audio_tags_ogg_packets_and_container_walks_are_bounded() {
    let mut bytes = ogg_page(
        0,
        2,
        42,
        &[30],
        &include_bytes!("../tests/fixtures/tagged-audio.ogg")[28..58],
    );
    for index in 0..5 {
        bytes.extend_from_slice(&ogg_page(
            index + 1,
            if index == 0 { 0 } else { 1 },
            42,
            &vec![255; 255],
            &vec![b'x'; 65025],
        ));
    }
    assert!(read_owned_bytes("packet.ogg", &bytes)
        .unwrap_err()
        .contains("256 KiB"));
    let chunks = vec![riff_chunk(b"JUNK", &[]); 4097];
    assert!(read_owned_bytes("chunks.wav", &riff_file(&chunks))
        .unwrap_err()
        .contains("4096"));
    let blocks = vec![(1, Vec::new()); 4096];
    assert!(read_owned_bytes("blocks.flac", &flac_file(&blocks))
        .unwrap_err()
        .contains("4096"));
    assert_eq!(
        read_owned_bytes(
            "recovery.ogg",
            include_bytes!("../tests/fixtures/tagged-audio.ogg")
        )
        .unwrap()
        .title
        .as_deref(),
        Some("Salzburg River")
    );
}

#[test]
fn embedded_audio_tags_trailing_id3v1_is_honestly_unsupported_with_or_without_v2() {
    let mut footer = vec![0; 128];
    footer[..3].copy_from_slice(b"TAG");
    footer[3..3 + 12].copy_from_slice(b"Footer title");
    footer[33..33 + 13].copy_from_slice(b"Footer artist");
    let original = include_bytes!("../tests/fixtures/tagged-audio.mp3");
    let mut v1_only = original[144..].to_vec();
    v1_only.extend_from_slice(&footer);
    let absent = read_owned_bytes("footer.mp3", &v1_only).unwrap();
    assert_eq!(absent.status, AudioTagStatus::Unsupported);
    assert_eq!(
        (absent.title, absent.artist, absent.album, absent.genre),
        (None, None, None, None)
    );
    assert!(absent.message.unwrap().contains("ID3v1"));
    let mut mixed = original.to_vec();
    mixed.extend_from_slice(&footer);
    let supported = read_owned_bytes("mixed.mp3", &mixed).unwrap();
    assert_eq!(supported.status, AudioTagStatus::Unsupported);
    assert_eq!(supported.title.as_deref(), Some("Salzburg River"));
    assert_eq!(supported.artist.as_deref(), Some("Owned Fixture"));
    assert!(supported.message.unwrap().contains("ID3v1"));
}
