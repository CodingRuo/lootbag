use lootbag::library::{AudioStatus, CatalogQuery, Library};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "game-assets-waveform-{}-{}",
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

fn oversized_vorbis_setup(cumulative: bool) -> Vec<u8> {
    use symphonia::core::{checksum::Crc32, io::Monitor};
    fn page(sequence: u32, flags: u8, packet: &[u8]) -> Vec<u8> {
        assert!(packet.len() < 255);
        let mut page = Vec::from(&b"OggS\0"[..]);
        page.push(flags);
        page.extend_from_slice(&0u64.to_le_bytes());
        page.extend_from_slice(&1u32.to_le_bytes());
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes());
        page.extend_from_slice(&[1, packet.len() as u8]);
        page.extend_from_slice(packet);
        let mut crc = Crc32::new(0);
        crc.process_buf_bytes(&page);
        page[22..26].copy_from_slice(&crc.crc().to_le_bytes());
        page
    }
    fn bits(output: &mut Vec<u8>, offset: &mut usize, value: u32, width: usize) {
        for bit in 0..width {
            if *offset / 8 == output.len() {
                output.push(0);
            }
            output[*offset / 8] |= (((value >> bit) & 1) as u8) << (*offset % 8);
            *offset += 1;
        }
    }
    let mut ident = Vec::from(&b"\x01vorbis"[..]);
    ident.extend_from_slice(&0u32.to_le_bytes());
    ident.push(1);
    ident.extend_from_slice(&8_000u32.to_le_bytes());
    ident.extend_from_slice(&[0; 12]);
    ident.extend_from_slice(&[0x66, 1]);
    let mut comment = Vec::from(&b"\x03vorbis"[..]);
    comment.extend_from_slice(&[0; 8]);
    comment.push(1);
    let mut setup = Vec::from(&b"\x05vorbis"[..]);
    let mut offset = setup.len() * 8;
    bits(&mut setup, &mut offset, u32::from(cumulative), 8);
    if cumulative {
        bits(&mut setup, &mut offset, 0x564342, 24);
        bits(&mut setup, &mut offset, 16, 16);
        bits(&mut setup, &mut offset, 32_768, 24);
        bits(&mut setup, &mut offset, 1, 1); // Ordered, 32768 complete 15-bit codes.
        bits(&mut setup, &mut offset, 14, 5);
        bits(&mut setup, &mut offset, 32_768, 16);
        bits(&mut setup, &mut offset, 0, 4); // Scalar: no expanded VQ data in test.
    }
    bits(&mut setup, &mut offset, 0x564342, 24);
    bits(
        &mut setup,
        &mut offset,
        if cumulative { 16 } else { 32 },
        16,
    );
    bits(
        &mut setup,
        &mut offset,
        if cumulative { 32_768 } else { 65_536 },
        24,
    );
    // Deliberately stop before this book's lengths/lookup. The unguarded decoder
    // can only fail on EOF; the test never supplies a complete allocation bomb.
    let mut ogg = page(0, 2, &ident);
    ogg.extend(page(1, 0, &comment));
    ogg.extend(page(2, 0, &setup));
    ogg.extend(page(3, 4, &[0]));
    ogg
}

#[test]
fn enormous_vorbis_headers_stop_with_a_visible_error() {
    use symphonia::core::{checksum::Crc32, io::Monitor};
    let workspace = Workspace::new();
    let source = workspace.0.join("sounds");
    fs::create_dir(&source).unwrap();
    let mut ogg = oversized_vorbis_setup(false)[..58].to_vec(); // Complete identification page.
    let mut comment = Vec::from(&b"\x03vorbis"[..]);
    comment.extend_from_slice(&(2 * 1024 * 1024u32 + 1024).to_le_bytes());
    comment.resize(11 + 2 * 1024 * 1024 + 1024, b'x');
    comment.extend_from_slice(&[0; 4]);
    comment.push(1);
    for (index, chunk) in comment.chunks(65_025).enumerate() {
        let mut segments = vec![255u8; chunk.len() / 255];
        if chunk.len() < 65_025 {
            segments.push((chunk.len() % 255) as u8);
        }
        let mut page = Vec::from(&b"OggS\0"[..]);
        page.push(u8::from(index > 0));
        page.extend_from_slice(&0u64.to_le_bytes());
        page.extend_from_slice(&1u32.to_le_bytes());
        page.extend_from_slice(&(index as u32 + 1).to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes());
        page.push(segments.len() as u8);
        page.extend_from_slice(&segments);
        page.extend_from_slice(chunk);
        let mut crc = Crc32::new(0);
        crc.process_buf_bytes(&page);
        page[22..26].copy_from_slice(&crc.crc().to_le_bytes());
        ogg.extend(page);
    }
    fs::write(source.join("long-header.ogg"), &ogg).unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    let id = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    let metadata = library.analyze_audio(id).unwrap();
    assert_eq!(metadata.status, AudioStatus::Error);
    assert!(library.audio_waveform(id, &AtomicBool::new(false)).is_err());
}

fn wav() -> Vec<u8> {
    // Two seconds: one silent second, then opposite polarity half-scale stereo.
    let mut bytes = Vec::from(&b"RIFF"[..]);
    let size = 8_000u32 * 2 * 2 * 2;
    bytes.extend_from_slice(&(36 + size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&8_000u32.to_le_bytes());
    bytes.extend_from_slice(&32_000u32.to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.resize(44 + 32_000, 0);
    for _ in 0..8_000 {
        bytes.extend_from_slice(&16_384i16.to_le_bytes());
        bytes.extend_from_slice(&(-16_384i16).to_le_bytes());
    }
    bytes
}

#[test]
fn waveform_retains_silence_timing_and_both_stereo_channels_without_changing_originals() {
    let workspace = Workspace::new();
    let source = workspace.0.join("sounds");
    fs::create_dir(&source).unwrap();
    let bytes = wav();
    fs::write(source.join("stereo.wav"), &bytes).unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    let id = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    let waveform = library.audio_waveform(id, &AtomicBool::new(false)).unwrap();
    assert_eq!(waveform.duration_seconds, 2.0);
    assert!(!waveform.peaks.is_empty());
    assert!(waveform.peaks.len() <= 512);
    assert_eq!(waveform.peaks[0].start_seconds, 0.0);
    assert_eq!(waveform.peaks.last().unwrap().end_seconds, 2.0);
    assert!(waveform
        .peaks
        .iter()
        .filter(|peak| peak.end_seconds <= 1.0)
        .all(|peak| peak.min == 0.0 && peak.max == 0.0));
    assert!(waveform
        .peaks
        .iter()
        .filter(|peak| peak.start_seconds >= 1.0)
        .all(|peak| peak.min == -0.5 && peak.max == 0.5));
    assert_eq!(fs::read(source.join("stereo.wav")).unwrap(), bytes);
    assert_eq!(
        fs::read(library.resolve_preview(id).unwrap()).unwrap(),
        bytes
    );
}

#[test]
fn real_compressed_sounds_have_full_timed_nonempty_waveforms() {
    let workspace = Workspace::new();
    let source = workspace.0.join("sounds");
    fs::create_dir(&source).unwrap();
    let fixtures: &[(&str, &[u8])] = &[
        ("tone.mp3", include_bytes!("fixtures/audio/stereo44100.mp3")),
        ("tone.ogg", include_bytes!("fixtures/audio/stereo32000.ogg")),
        (
            "tone.flac",
            include_bytes!("fixtures/audio/stereo48000.flac"),
        ),
    ];
    for (name, bytes) in fixtures {
        fs::write(source.join(name), bytes).unwrap();
    }
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    for file in library
        .query(CatalogQuery::default())
        .unwrap()
        .items
        .iter()
        .flat_map(|asset| &asset.files)
    {
        let waveform = library
            .audio_waveform(file.id, &AtomicBool::new(false))
            .unwrap();
        assert!(
            (waveform.duration_seconds - 0.25).abs() < 0.001,
            "{}: {}",
            file.format,
            waveform.duration_seconds
        );
        assert_eq!(waveform.peaks[0].start_seconds, 0.0);
        assert_eq!(
            waveform.peaks.last().unwrap().end_seconds,
            waveform.duration_seconds
        );
        assert!(waveform.peaks.len() <= 512);
        assert!(waveform
            .peaks
            .iter()
            .any(|peak| peak.min < -0.01 && peak.max > 0.01));
        for pair in waveform.peaks.windows(2) {
            assert_eq!(pair[0].end_seconds, pair[1].start_seconds);
        }
        assert_eq!(
            fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
            fixtures
                .iter()
                .find(|(name, _)| *name == file.original_filename)
                .unwrap()
                .1
        );
    }
}

#[test]
fn long_waveforms_remain_bounded_without_losing_timing_or_non_silent_regions() {
    let workspace = Workspace::new();
    let source = workspace.0.join("sounds");
    fs::create_dir(&source).unwrap();
    let mut bytes = wav();
    bytes.resize(44 + 320_000, 0);
    bytes[4..8].copy_from_slice(&320_036u32.to_le_bytes());
    bytes[40..44].copy_from_slice(&320_000u32.to_le_bytes());
    fs::write(source.join("long.wav"), &bytes).unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    let id = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    let waveform = library.audio_waveform(id, &AtomicBool::new(false)).unwrap();
    assert_eq!(waveform.duration_seconds, 10.0);
    assert!(waveform.peaks.len() <= 512);
    assert_eq!(waveform.peaks.last().unwrap().end_seconds, 10.0);
    assert!(waveform
        .peaks
        .iter()
        .any(|peak| peak.start_seconds >= 1.0 && peak.end_seconds <= 2.0 && peak.max == 0.5));
    assert!(waveform
        .peaks
        .iter()
        .filter(|peak| peak.start_seconds >= 2.0)
        .all(|peak| peak.min == 0.0 && peak.max == 0.0));
}

#[test]
fn waveform_errors_and_cancellation_do_not_cache_failures_or_mutate_originals() {
    let workspace = Workspace::new();
    let source = workspace.0.join("sounds");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("valid.wav"), wav()).unwrap();
    fs::write(source.join("broken.wav"), b"not a sound").unwrap();
    fs::write(
        source.join("unsupported.ogg"),
        include_bytes!("fixtures/audio/unsupported-ogg-flac.ogg"),
    )
    .unwrap();
    fs::write(source.join("image.png"), b"not audio").unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    let page = library.query(CatalogQuery::default()).unwrap();
    for file in page.items.iter().flat_map(|asset| &asset.files) {
        let cancel = AtomicBool::new(file.original_filename == "valid.wav");
        let error = library.audio_waveform(file.id, &cancel).unwrap_err();
        assert!(!error.is_empty());
        if file.original_filename == "valid.wav" {
            assert!(error.contains("abgebrochen"));
        }
        if file.original_filename == "image.png" {
            assert!(error.contains("kein Sound"));
        }
        assert_eq!(
            fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
            fs::read(source.join(&file.original_filename)).unwrap()
        );
    }
    assert!(library
        .query(CatalogQuery::default())
        .unwrap()
        .items
        .iter()
        .flat_map(|asset| &asset.files)
        .all(|file| file.audio.is_none()));
}

#[test]
fn referenced_waveforms_reject_changed_missing_oversized_and_trashed_sources() {
    let workspace = Workspace::new();
    let source = workspace.0.join("sounds");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("valid.wav"), wav()).unwrap();
    let large = fs::File::create(source.join("large.wav")).unwrap();
    large.set_len(512 * 1024 * 1024 + 1).unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.reference_folder(&source).unwrap();
    let page = library.query(CatalogQuery::default()).unwrap();
    let id = page
        .items
        .iter()
        .flat_map(|asset| &asset.files)
        .find(|file| file.original_filename == "valid.wav")
        .unwrap()
        .id;
    let large_id = page
        .items
        .iter()
        .flat_map(|asset| &asset.files)
        .find(|file| file.original_filename == "large.wav")
        .unwrap()
        .id;
    assert!(library.audio_waveform(id, &AtomicBool::new(false)).is_ok());
    assert!(library
        .audio_waveform(large_id, &AtomicBool::new(false))
        .unwrap_err()
        .contains("512 MiB"));
    fs::write(source.join("valid.wav"), b"changed bytes").unwrap();
    assert!(library.audio_waveform(id, &AtomicBool::new(false)).is_err());
    fs::remove_file(source.join("valid.wav")).unwrap();
    assert!(library.audio_waveform(id, &AtomicBool::new(false)).is_err());
    library.trash_file(large_id).unwrap();
    assert!(library
        .audio_waveform(large_id, &AtomicBool::new(false))
        .is_err());
}

#[test]
fn oversized_vorbis_codebooks_are_rejected_without_changing_originals() {
    let workspace = Workspace::new();
    let source = workspace.0.join("sounds");
    fs::create_dir(&source).unwrap();
    for (name, cumulative) in [("one.ogg", false), ("cumulative.ogg", true)] {
        fs::write(source.join(name), oversized_vorbis_setup(cumulative)).unwrap();
    }
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    for file in library
        .query(CatalogQuery::default())
        .unwrap()
        .items
        .iter()
        .flat_map(|asset| &asset.files)
    {
        let metadata = library.analyze_audio(file.id).unwrap();
        assert_eq!(metadata.status, AudioStatus::Error);
        assert!(library
            .audio_waveform(file.id, &AtomicBool::new(false))
            .is_err());
        assert_eq!(
            fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
            fs::read(source.join(&file.original_filename)).unwrap()
        );
    }
}

#[test]
fn malformed_vorbis_lookup_returns_visible_metadata_and_waveform_errors_without_unwinding() {
    let workspace = Workspace::new();
    let source = workspace.0.join("sounds");
    fs::create_dir(&source).unwrap();
    let bytes = include_bytes!("fixtures/audio/malformed-vorbis-lookup.ogg");
    fs::write(source.join("malformed.ogg"), bytes).unwrap();
    let mut library = Library::create(&workspace.0.join("library")).unwrap();
    library.import_folder(&source).unwrap();
    let id = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    let metadata = library.analyze_audio(id).unwrap();
    assert_eq!(metadata.status, AudioStatus::Error);
    assert!(metadata.error.unwrap().contains("Codec-Parserfehler"));
    assert!(library
        .audio_waveform(id, &AtomicBool::new(false))
        .unwrap_err()
        .contains("Codec-Parserfehler"));
    let saved = library.query(CatalogQuery::default()).unwrap().items[0].files[0]
        .audio
        .clone()
        .unwrap();
    assert_eq!(saved.status, AudioStatus::Error);
    assert!(saved.error.unwrap().contains("Codec-Parserfehler"));
    assert_eq!(fs::read(source.join("malformed.ogg")).unwrap(), bytes);
    assert_eq!(
        fs::read(library.resolve_preview(id).unwrap()).unwrap(),
        bytes
    );
}
