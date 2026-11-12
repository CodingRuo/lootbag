use super::{AssetKind, Library, Result};
#[path = "audio_guard.rs"]
pub(super) mod header_guard;
#[path = "vorbis_guard.rs"]
mod vorbis_guard;
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use symphonia::core::{
    codecs::audio::well_known::{
        CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_PCM_F64BE_PLANAR, CODEC_ID_PCM_S32LE, CODEC_ID_VORBIS,
    },
    formats::{
        probe::{Hint, Probe},
        FormatReader, TrackType,
    },
    io::{MediaSource, MediaSourceStream},
};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioStatus {
    Ready,
    Error,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMetadata {
    pub status: AudioStatus,
    pub duration_seconds: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub codec: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformPeak {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub min: f32,
    pub max: f32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioWaveform {
    pub duration_seconds: f64,
    pub peaks: Vec<WaveformPeak>,
}

// ponytail: bounded audition, not an audio editor; revisit these limits for multi-hour recordings.
const MAX_AUDIO_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DECODED_SAMPLES: u64 = 200_000_000;
const MAX_PACKET_BYTES: usize = 1024 * 1024;
const MAX_BUFFER_SAMPLES: usize = 1024 * 1024;
const MAX_PEAKS: usize = 512;
const MAX_WORK: Duration = Duration::from_secs(30);
const MAX_HEADER_READ: usize = 2 * 1024 * 1024;

struct HeaderSource<'a> {
    file: File,
    read: usize,
    ready: Arc<AtomicBool>,
    cancelled: &'a AtomicBool,
    started: Instant,
    limit: usize,
    length: Option<u64>,
}
impl HeaderSource<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "Audioanalyse abgebrochen",
            ))
        } else if self.started.elapsed() > MAX_WORK {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Audioanalyse überschreitet 30 Sekunden",
            ))
        } else {
            Ok(())
        }
    }
}
impl Read for HeaderSource<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        if output.is_empty() {
            return Ok(0);
        }
        let cap = if self.ready.load(Ordering::Relaxed) {
            self.limit
        } else {
            MAX_HEADER_READ
        };
        let remaining = cap.saturating_sub(self.read);
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Audioanalyse überschreitet ihre kumulative Lesegrenze",
            ));
        }
        let length = remaining.min(output.len());
        let count = self.file.read(&mut output[..length])?;
        self.read += count;
        Ok(count)
    }
}
impl Seek for HeaderSource<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.check()?;
        self.file.seek(position)
    }
}
impl MediaSource for HeaderSource<'_> {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.length
    }
}
fn probe_stream<'a>(
    file: File,
    cancelled: &'a AtomicBool,
    waveform: bool,
) -> Result<(MediaSourceStream<'a>, Arc<AtomicBool>)> {
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let ready = Arc::new(AtomicBool::new(false));
    let mut source = HeaderSource {
        file,
        read: 0,
        ready: ready.clone(),
        cancelled,
        started: Instant::now(),
        limit: if waveform {
            MAX_AUDIO_BYTES as usize
        } else {
            8 * 1024 * 1024
        },
        length: Some(size),
    };
    let mut signature = [0; 4];
    source
        .read_exact(&mut signature)
        .map_err(|e| e.to_string())?;
    // The MP3 reader otherwise exposes its bitrate estimate as an ordinary duration.
    if !matches!(&signature, b"RIFF" | b"OggS" | b"fLaC") {
        source.length = None;
    }
    source.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    header_guard::validate(&mut source, size, &signature)?;
    source.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    Ok((
        MediaSourceStream::new(Box::new(source), Default::default()),
        ready,
    ))
}

fn probe<'a>(path: &Path, stream: MediaSourceStream<'a>) -> Result<Box<dyn FormatReader + 'a>> {
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    // A failing external parser and its owned stream are discarded; no shared mutable state is reused.
    catch_unwind(AssertUnwindSafe(|| {
        // Keep audition format-only when private source-tag readers are enabled.
        let mut probe = Probe::default();
        probe.register_format::<symphonia::default::formats::WavReader<'_>>();
        probe.register_format::<symphonia::default::formats::MpaReader<'_>>();
        probe.register_format::<symphonia::default::formats::OggReader<'_>>();
        probe.register_format::<symphonia::default::formats::FlacReader<'_>>();
        probe.probe(&hint, stream, Default::default(), Default::default())
    }))
    .map_err(|_| "Audioheader konnte nicht sicher gelesen werden: Codec-Parserfehler".to_string())?
    .map_err(|error| format!("Audioformat nicht lesbar oder nicht unterstützt: {error}"))
}

impl Library {
    pub fn audio_waveform(&self, file_id: i64, cancelled: &AtomicBool) -> Result<AudioWaveform> {
        let path = self.resolve_preview(file_id)?;
        let format: String = self
            .connection
            .query_row(
                "SELECT format FROM active_asset_files WHERE id = ?1",
                [file_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("Asset-Datei nicht im Katalog: {error}"))?;
        if AssetKind::for_format(&format) != Some(AssetKind::Sound) {
            return Err("Diese Asset-Datei ist kein Sound".into());
        }
        let source = open_source(&path)?;
        let guard = source.try_clone().map_err(|error| error.to_string())?;
        let before = source_stamp(&guard.metadata().map_err(|error| error.to_string())?)?;
        if before != source_stamp(&fs::symlink_metadata(&path).map_err(|error| error.to_string())?)?
        {
            return Err("Sound wurde vor der Analyse ersetzt".into());
        }
        if before.0 > MAX_AUDIO_BYTES {
            return Err("Wellenform nicht verfügbar: Die Datei überschreitet 512 MiB. Wiedergabe bleibt möglich.".into());
        }
        let waveform = waveform(source, &path, cancelled)?;
        let resolved = self.resolve_preview(file_id)?;
        if resolved != path
            || before != source_stamp(&guard.metadata().map_err(|error| error.to_string())?)?
            || before
                != source_stamp(
                    &fs::symlink_metadata(resolved).map_err(|error| error.to_string())?,
                )?
        {
            return Err("Sound wurde während der Wellenform-Analyse geändert".into());
        }
        Ok(waveform)
    }
}

pub(super) fn open_source(path: &std::path::Path) -> Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_vendor = "apple")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(4 | 0x100); // SDK O_NONBLOCK | O_NOFOLLOW.
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x800 | 0x20000); // Linux x86_64 O_NONBLOCK | O_NOFOLLOW.
    }
    #[cfg(all(
        unix,
        not(any(
            target_vendor = "apple",
            all(target_os = "linux", target_arch = "x86_64")
        ))
    ))]
    return Err("Sicheres Audio-Lesen ist auf dieser Unix-Plattform noch nicht verfügbar".into());
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT.
    }
    let file = options
        .open(path)
        .map_err(|error| format!("Sound nicht lesbar: {error}"))?;
    source_stamp(&file.metadata().map_err(|error| error.to_string())?)?;
    Ok(file)
}

pub(super) fn source_stamp(
    metadata: &fs::Metadata,
) -> Result<(u64, std::time::SystemTime, String)> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Sound wurde durch eine Verknüpfung ersetzt".into());
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = format!(
        "{:?}",
        metadata.created().map_err(|error| error.to_string())?
    );
    Ok((
        metadata.len(),
        metadata.modified().map_err(|error| error.to_string())?,
        identity,
    ))
}

fn validate_parameters(
    parameters: &symphonia::core::codecs::audio::AudioCodecParameters,
) -> Result<()> {
    if parameters
        .channels
        .as_ref()
        .is_some_and(|channels| channels.count() == 0 || channels.count() > 8)
        || parameters
            .sample_rate
            .is_some_and(|rate| rate == 0 || rate > 192_000)
        || parameters
            .frames_per_block
            .is_some_and(|frames| frames > MAX_BUFFER_SAMPLES as u64 / 8)
        || parameters
            .max_frames_per_packet
            .is_some_and(|frames| frames > MAX_BUFFER_SAMPLES as u64 / 8)
    {
        return Err("Sound überschreitet die Grenzen der Audioanalyse".into());
    }
    Ok(())
}

fn waveform(source: File, path: &Path, cancelled: &AtomicBool) -> Result<AudioWaveform> {
    let started = Instant::now();
    let check = || {
        if cancelled.load(Ordering::Relaxed) {
            Err("Wellenform-Analyse abgebrochen".to_string())
        } else if started.elapsed() > MAX_WORK {
            Err(
                "Wellenform-Analyse überschreitet 30 Sekunden. Wiedergabe bleibt möglich."
                    .to_string(),
            )
        } else {
            Ok(())
        }
    };
    check()?;
    let (stream, headers_ready) = probe_stream(source, cancelled, true)?;
    let mut format = probe(path, stream)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("Keine lesbare Audiospur gefunden")?;
    let parameters = track
        .codec_params
        .as_ref()
        .and_then(|value| value.audio())
        .ok_or("Audioeigenschaften fehlen")?;
    if !supported_codec(format.format_info().short_name, parameters.codec) {
        return Err("Audiocodec wird für die Wellenform nicht unterstützt".into());
    }
    validate_parameters(parameters)?;
    vorbis_guard::validate(parameters)?;
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(parameters, &Default::default())
        .map_err(|error| format!("Audiocodec nicht unterstützt: {error}"))?;
    headers_ready.store(true, Ordering::Relaxed);
    let mut peaks: Vec<WaveformPeak> = Vec::with_capacity(MAX_PEAKS);
    let mut samples = Vec::<f32>::new();
    let mut frames = 0u64;
    let mut window = 0u64;
    let mut window_start = 0u64;
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut rate = 0u32;
    let mut decoded_samples = 0u64;
    for _ in 0..1_000_000 {
        check()?;
        let Some(packet) = format
            .next_packet()
            .map_err(|error| format!("Sound beschädigt oder nicht lesbar: {error}"))?
        else {
            if frames == 0 {
                return Err("Sound enthält keine lesbaren Audiodaten".into());
            }
            if window_start < frames {
                peaks.push(WaveformPeak {
                    start_seconds: window_start as f64 / rate as f64,
                    end_seconds: frames as f64 / rate as f64,
                    min,
                    max,
                });
            }
            return Ok(AudioWaveform {
                duration_seconds: frames as f64 / rate as f64,
                peaks,
            });
        };
        if packet.data.len() > MAX_PACKET_BYTES {
            return Err("Audiopaket überschreitet die Wellenform-Grenze".into());
        }
        if packet.track_id != id {
            continue;
        }
        let decoded = decoder
            .decode(&packet)
            .map_err(|error| format!("Sound beschädigt oder nicht lesbar: {error}"))?;
        let channels = decoded.spec().channels().count();
        let packet_rate = decoded.spec().rate();
        let count = decoded
            .frames()
            .checked_mul(channels)
            .ok_or("Audiobuffer zu groß")?;
        if count > MAX_BUFFER_SAMPLES
            || channels == 0
            || channels > 8
            || packet_rate == 0
            || packet_rate > 192_000
        {
            return Err("Audiobuffer überschreitet die Wellenform-Grenze".into());
        }
        if rate == 0 {
            rate = packet_rate;
            window = (rate as u64 / 200).max(1);
        }
        if packet_rate != rate {
            return Err("Wechselnde Samplerate wird für Wellenformen nicht unterstützt".into());
        }
        decoded_samples += count as u64;
        if decoded_samples > MAX_DECODED_SAMPLES {
            return Err(
                "Sound überschreitet 200 Millionen Samples. Wiedergabe bleibt möglich.".into(),
            );
        }
        decoded.slice(..).copy_to_vec_interleaved(&mut samples);
        for frame in samples.chunks_exact(channels) {
            for sample in frame {
                if !sample.is_finite() {
                    return Err("Sound enthält ungültige Samples".into());
                }
                min = min.min(sample.clamp(-1.0, 1.0));
                max = max.max(sample.clamp(-1.0, 1.0));
            }
            frames += 1;
            if frames - window_start == window {
                peaks.push(WaveformPeak {
                    start_seconds: window_start as f64 / rate as f64,
                    end_seconds: frames as f64 / rate as f64,
                    min,
                    max,
                });
                window_start = frames;
                min = f32::INFINITY;
                max = f32::NEG_INFINITY;
                if peaks.len() == MAX_PEAKS {
                    for index in 0..MAX_PEAKS / 2 {
                        let left = &peaks[index * 2];
                        let right = &peaks[index * 2 + 1];
                        peaks[index] = WaveformPeak {
                            start_seconds: left.start_seconds,
                            end_seconds: right.end_seconds,
                            min: left.min.min(right.min),
                            max: left.max.max(right.max),
                        };
                    }
                    peaks.truncate(MAX_PEAKS / 2);
                    window *= 2;
                }
            }
        }
    }
    Err("Sound enthält zu viele Audiopakete für die Wellenform-Analyse".into())
}

fn supported_codec(format: &str, codec: symphonia::core::codecs::audio::AudioCodecId) -> bool {
    match format {
        "wave" => (CODEC_ID_PCM_S32LE..=CODEC_ID_PCM_F64BE_PLANAR).contains(&codec),
        "mp3" => codec == CODEC_ID_MP3,
        "ogg" => codec == CODEC_ID_VORBIS,
        "flac" => codec == CODEC_ID_FLAC,
        _ => false,
    }
}

fn inspect(source: File, path: &Path, cancelled: &AtomicBool) -> Result<AudioMetadata> {
    let started = Instant::now();
    if cancelled.load(Ordering::Relaxed) {
        return Err("Audioanalyse abgebrochen".into());
    }
    let (stream, headers_ready) = probe_stream(source, cancelled, false)?;
    let mut format = probe(path, stream)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("Keine lesbare Audiospur gefunden")?;
    let parameters = track
        .codec_params
        .as_ref()
        .and_then(|value| value.audio())
        .ok_or("Audioeigenschaften fehlen")?;
    if !supported_codec(format.format_info().short_name, parameters.codec) {
        return Err("Audiocodec wird nicht unterstützt. Vorschauziele: WAV/PCM, MP3, OGG/Vorbis und native FLAC.".into());
    }
    validate_parameters(parameters)?;
    vorbis_guard::validate(parameters)?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(parameters, &Default::default())
        .map_err(|error| format!("Audiocodec nicht unterstützt: {error}"))?;
    headers_ready.store(true, Ordering::Relaxed);
    let mut metadata = AudioMetadata {
        status: AudioStatus::Ready,
        duration_seconds: track
            .duration
            .zip(track.time_base)
            .and_then(|(duration, time_base)| time_base.calc_duration(duration))
            .map(|time| time.as_secs_f64()),
        sample_rate: parameters.sample_rate,
        channels: parameters
            .channels
            .as_ref()
            .map(|value| value.count() as u32),
        codec: Some(decoder.codec_info().long_name.to_owned()),
        error: None,
    };
    let track_id = track.id;
    // ponytail: checking the first decoded packet is bounded; the native player reports later playback failures.
    for _ in 0..64 {
        if started.elapsed() > MAX_WORK {
            return Err("Audioanalyse überschreitet 30 Sekunden".into());
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err("Audioanalyse abgebrochen".into());
        }
        let packet = format
            .next_packet()
            .map_err(|error| format!("Sound ist beschädigt oder nicht lesbar: {error}"))?
            .ok_or("Sound enthält keine lesbaren Audiodaten")?;
        if packet.data.len() > MAX_PACKET_BYTES {
            return Err("Audiopaket überschreitet die Analysegrenze".into());
        }
        if packet.track_id != track_id {
            continue;
        }
        let decoded = decoder
            .decode(&packet)
            .map_err(|error| format!("Sound ist beschädigt oder nicht lesbar: {error}"))?;
        if started.elapsed() > MAX_WORK {
            return Err("Audioanalyse überschreitet 30 Sekunden".into());
        }
        let count = decoded
            .frames()
            .checked_mul(decoded.spec().channels().count())
            .ok_or("Audiobuffer zu groß")?;
        if count > MAX_BUFFER_SAMPLES
            || decoded.spec().channels().count() == 0
            || decoded.spec().channels().count() > 8
            || decoded.spec().rate() == 0
            || decoded.spec().rate() > 192_000
        {
            return Err("Audiobuffer überschreitet die Analysegrenze".into());
        }
        if decoded.frames() > 0 {
            // Updated decoder parameters still describe the source codec; no resampled playback buffer is used.
            let parameters = decoder.codec_params();
            metadata.sample_rate = metadata.sample_rate.or(parameters.sample_rate);
            metadata.channels = metadata.channels.or_else(|| {
                parameters
                    .channels
                    .as_ref()
                    .map(|value| value.count() as u32)
            });
            return Ok(metadata);
        }
    }
    Err("Sound enthält keine lesbaren Audiodaten".into())
}

pub(crate) struct AudioInspection {
    path: PathBuf,
    file: File,
    stamp: (u64, std::time::SystemTime, String),
    metadata: AudioMetadata,
}
impl Library {
    pub(crate) fn inspect_audio(
        &self,
        file_id: i64,
        cancelled: &AtomicBool,
    ) -> Result<AudioInspection> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Audioanalyse abgebrochen".into());
        }
        let path = self.resolve_preview(file_id)?;
        let format: String = self
            .connection
            .query_row(
                "SELECT format FROM active_asset_files WHERE id = ?1",
                [file_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if AssetKind::for_format(&format) != Some(AssetKind::Sound) {
            return Err("Diese Asset-Datei ist kein Sound".into());
        }
        let file = open_source(&path)?;
        let stamp = source_stamp(&file.metadata().map_err(|e| e.to_string())?)?;
        if stamp != source_stamp(&fs::symlink_metadata(&path).map_err(|e| e.to_string())?)? {
            return Err("Sound wurde vor der Analyse ersetzt".into());
        }
        let metadata = match catch_unwind(AssertUnwindSafe(|| {
            if stamp.0 > MAX_AUDIO_BYTES {
                return Err("Audioanalyse ist auf 512 MiB pro Datei begrenzt".into());
            }
            inspect(
                file.try_clone().map_err(|e| e.to_string())?,
                &path,
                cancelled,
            )
        })) {
            Ok(Ok(metadata)) => metadata,
            result => AudioMetadata {
                status: AudioStatus::Error,
                duration_seconds: None,
                sample_rate: None,
                channels: None,
                codec: None,
                error: Some(match result {
                    Ok(Err(error)) => error,
                    _ => "Audioanalyse: Codec-Parserfehler".into(),
                }),
            },
        };
        if cancelled.load(Ordering::Relaxed) {
            return Err("Audioanalyse abgebrochen".into());
        }
        Ok(AudioInspection {
            path,
            file,
            stamp,
            metadata,
        })
    }
    pub(crate) fn store_audio_inspection(
        &self,
        file_id: i64,
        result: AudioInspection,
        cancelled: &AtomicBool,
    ) -> Result<AudioMetadata> {
        // Acquire the writer first: cancellation, source and trash may change while SQLite was busy.
        let transaction = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(|error| format!("Audioeigenschaften können nicht gespeichert werden: {error}"))?;
        result.validate(self, file_id, cancelled)?;
        let metadata = &result.metadata;
        transaction.execute(
            "INSERT INTO audio_metadata(file_id, duration_seconds, sample_rate, channels, codec, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(file_id) DO UPDATE SET duration_seconds = excluded.duration_seconds, sample_rate = excluded.sample_rate, channels = excluded.channels, codec = excluded.codec, error = excluded.error",
            rusqlite::params![file_id, metadata.duration_seconds, metadata.sample_rate, metadata.channels, metadata.codec, metadata.error],
        ).map_err(|error| format!("Audioeigenschaften können nicht gespeichert werden: {error}"))?;
        result.validate(self, file_id, cancelled)?;
        transaction.commit().map_err(|error| {
            format!("Audioeigenschaften können nicht gespeichert werden: {error}")
        })?;
        Ok(result.metadata)
    }
}
impl AudioInspection {
    fn validate(&self, library: &Library, file_id: i64, cancelled: &AtomicBool) -> Result<()> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Audioanalyse abgebrochen".into());
        }
        let resolved = library.resolve_preview(file_id)?;
        if resolved != self.path
            || self.stamp != source_stamp(&self.file.metadata().map_err(|e| e.to_string())?)?
            || self.stamp
                != source_stamp(&fs::symlink_metadata(resolved).map_err(|e| e.to_string())?)?
        {
            return Err("Sound wurde während der Audioanalyse geändert".into());
        }
        Ok(())
    }
}
