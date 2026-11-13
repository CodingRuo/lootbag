use super::{
    audio::{open_source, source_stamp},
    AssetKind, Library, Result,
};
use std::{
    fs::{self, File},
    io::{self, Cursor, Read, Seek, SeekFrom},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use symphonia::core::{
    checksum::Crc32,
    io::{MediaSourceStream, Monitor},
    meta::{MetadataBuilder, MetadataInfo, MetadataReader, MetadataRevision, StandardTag},
};

const MAX_READ: usize = 2 * 1024 * 1024;
const MAX_TAG_BYTES: usize = 256 * 1024;
const MAX_FIELD_BYTES: usize = 4 * 1024;
const MAX_TEXT_BYTES: usize = 1024;
const MAX_ENTRIES: usize = 256;
const MAX_CHUNKS: usize = 4096;
const MAX_WORK: Duration = Duration::from_secs(3);

#[derive(Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AudioTagStatus {
    #[default]
    Ready,
    Unsupported,
}

#[derive(Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmbeddedAudioTags {
    pub status: AudioTagStatus,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub message: Option<String>,
}

impl EmbeddedAudioTags {
    fn unsupported(message: &str) -> Self {
        Self {
            status: AudioTagStatus::Unsupported,
            message: Some(message.into()),
            ..Self::default()
        }
    }
    fn add(
        &mut self,
        revision: &MetadataRevision,
        track: Option<u64>,
        entries: &mut usize,
    ) -> Result<()> {
        for container in std::iter::once(&revision.media).chain(
            revision
                .per_track
                .iter()
                .filter(|item| Some(item.track_id) == track)
                .map(|item| &item.metadata),
        ) {
            for tag in &container.tags {
                *entries += 1;
                if *entries > MAX_ENTRIES {
                    return Err("Eingebettete Audio-Tags überschreiten 256 Einträge".into());
                }
                let (target, value) = match &tag.std {
                    Some(StandardTag::TrackTitle(value)) => (&mut self.title, value),
                    Some(StandardTag::Artist(value)) => (&mut self.artist, value),
                    Some(StandardTag::Album(value)) => (&mut self.album, value),
                    Some(StandardTag::Genre(value)) => (&mut self.genre, value),
                    _ => continue,
                };
                let value = value.trim_end_matches('\0');
                if value.len() > MAX_TEXT_BYTES {
                    return Err("Ein eingebetteter Audio-Tag überschreitet 1 KiB Text".into());
                }
                if !value.is_empty() && target.is_none() {
                    *target = Some(value.to_owned());
                }
            }
        }
        Ok(())
    }
}

struct TagSource<'a> {
    file: File,
    read: usize,
    cancelled: &'a AtomicBool,
    started: Instant,
}
impl TagSource<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(io::Error::other("Audio-Tag-Analyse abgebrochen"))
        } else if self.started.elapsed() > MAX_WORK {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Audio-Tag-Analyse überschreitet 3 Sekunden",
            ))
        } else {
            Ok(())
        }
    }
}
impl Read for TagSource<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        if output.is_empty() {
            return Ok(0);
        }
        let remaining = MAX_READ - self.read;
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Audio-Tags überschreiten die Lesegrenze von 2 MiB",
            ));
        }
        let length = output.len().min(remaining);
        let count = self.file.read(&mut output[..length])?;
        self.read += count;
        Ok(count)
    }
}
impl Seek for TagSource<'_> {
    fn seek(&mut self, at: SeekFrom) -> io::Result<u64> {
        self.check()?;
        self.file.seek(at)
    }
}

impl Library {
    pub(crate) fn embedded_audio_tags(
        &self,
        file_id: i64,
        cancelled: &AtomicBool,
    ) -> Result<EmbeddedAudioTags> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Audio-Tag-Analyse abgebrochen".into());
        }
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
            return Err("Sound wurde vor der Tag-Analyse ersetzt".into());
        }
        let mut source = TagSource {
            file: source,
            read: 0,
            cancelled,
            started: Instant::now(),
        };
        let tags = match format.as_str() {
            "WAV" => wav(&mut source),
            "MP3" => mp3(&mut source),
            "OGG" => ogg(&mut source),
            "FLAC" => flac(&mut source),
            _ => Ok(EmbeddedAudioTags::unsupported(
                "Eingebettete Tags dieses Soundformats werden nicht unterstützt",
            )),
        }?;
        source.check().map_err(|error| error.to_string())?;
        let resolved = self.resolve_preview(file_id)?;
        if resolved != path
            || before != source_stamp(&guard.metadata().map_err(|error| error.to_string())?)?
            || before
                != source_stamp(
                    &fs::symlink_metadata(resolved).map_err(|error| error.to_string())?,
                )?
        {
            return Err("Sound wurde während der Audio-Tag-Analyse geändert".into());
        }
        Ok(tags)
    }
}

fn read_exact<const N: usize>(source: &mut TagSource<'_>) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    source
        .read_exact(&mut bytes)
        .map_err(|error| format!("Audio-Tags nicht lesbar: {error}"))?;
    Ok(bytes)
}

fn chunk(source: &mut TagSource<'_>, end: u64) -> Result<Option<([u8; 4], usize, u64)>> {
    let pos = source
        .stream_position()
        .map_err(|error| error.to_string())?;
    if pos == end {
        return Ok(None);
    }
    if end.saturating_sub(pos) < 8 {
        return Err("RIFF-Tag-Header ist unvollständig".into());
    }
    let header = read_exact::<8>(source)?;
    let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    let data_end = pos
        .checked_add(8 + size as u64)
        .filter(|data_end| *data_end <= end)
        .ok_or("RIFF-Tag überschreitet den Container")?;
    // A final odd-sized chunk may omit its pad byte at the end of the container.
    let next = (data_end + (size & 1) as u64).min(end);
    Ok(Some((header[..4].try_into().unwrap(), size, next)))
}

fn wav(source: &mut TagSource<'_>) -> Result<EmbeddedAudioTags> {
    let header = read_exact::<12>(source)?;
    if &header[..4] == b"RF64" || &header[..4] == b"RIFX" {
        return Ok(EmbeddedAudioTags::unsupported(
            "Tags in RF64/RIFX-WAV werden noch nicht unterstützt",
        ));
    }
    if &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("WAV-Tag-Container ist nicht lesbar".into());
    }
    let end = 8 + u64::from(u32::from_le_bytes(header[4..8].try_into().unwrap()));
    let length = source
        .file
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    if end < 12 || end > length {
        return Err("RIFF-Tag-Container ist unvollständig".into());
    }
    let mut builder = MetadataBuilder::new(MetadataInfo {
        metadata: symphonia::core::meta::well_known::METADATA_ID_WAVE,
        short_name: "wave",
        long_name: "Waveform Audio File Format",
    });
    let mut count = 0usize;
    let mut bytes = 0usize;
    let mut tags = EmbeddedAudioTags::default();
    // ponytail: Symphonia 0.6.1 drops WAV INFO; remove this bounded framing once its reader retains INFO.
    for index in 0..=MAX_CHUNKS {
        let Some((id, size, next)) = chunk(source, end)? else {
            break;
        };
        if index == MAX_CHUNKS {
            return Err("RIFF-Tags überschreiten 4096 Container-Einträge".into());
        }
        if &id == b"LIST" {
            if size < 4 {
                return Err("RIFF-LIST-Tag ist unvollständig".into());
            }
            let list_end = source
                .stream_position()
                .map_err(|error| error.to_string())?
                + size as u64;
            let form = read_exact::<4>(source)?;
            if &form == b"INFO" {
                while let Some((field, len, following)) = chunk(source, list_end)? {
                    count += 1;
                    bytes = bytes.checked_add(len).ok_or("RIFF-Tags sind zu groß")?;
                    if count > MAX_ENTRIES || len > MAX_FIELD_BYTES || bytes > MAX_TAG_BYTES {
                        return Err(
                            "RIFF-Tags überschreiten die Grenzen für Einträge oder Textgröße"
                                .into(),
                        );
                    }
                    let mut value = vec![0; len];
                    source
                        .read_exact(&mut value)
                        .map_err(|error| format!("RIFF-Tag nicht lesbar: {error}"))?;
                    std::str::from_utf8(&value).map_err(|_| "RIFF-Tag enthält nicht unterstützten UTF-8-Text oder eine andere Zeichenkodierung")?;
                    symphonia::default::meta::embedded::riff::parse_riff_info_chunk(
                        field,
                        &value,
                        &mut builder,
                    )
                    .map_err(|error| format!("RIFF-Tag nicht lesbar: {error}"))?;
                    source
                        .seek(SeekFrom::Start(following))
                        .map_err(|error| error.to_string())?;
                }
            }
        } else if &id == b"id3 " || &id == b"ID3 " {
            tags.status = AudioTagStatus::Unsupported;
            tags.message =
                Some("Zusätzliche ID3-Tags im WAV-Container werden noch nicht unterstützt".into());
        }
        source
            .seek(SeekFrom::Start(next))
            .map_err(|error| error.to_string())?;
    }
    tags.add(&builder.build(), None, &mut 0)?;
    Ok(tags)
}

fn body(source: &mut TagSource<'_>, size: usize) -> Result<Vec<u8>> {
    if size > MAX_TAG_BYTES {
        return Err("Audio-Tags überschreiten 256 KiB Quelldaten".into());
    }
    let mut bytes = vec![0; size];
    source
        .read_exact(&mut bytes)
        .map_err(|error| format!("Audio-Tags nicht lesbar: {error}"))?;
    Ok(bytes)
}

fn take<'a>(bytes: &'a [u8], pos: &mut usize, size: usize) -> Result<&'a [u8]> {
    let end = pos
        .checked_add(size)
        .filter(|end| *end <= bytes.len())
        .ok_or("Audio-Tag-Struktur ist unvollständig")?;
    let value = &bytes[*pos..end];
    *pos = end;
    Ok(value)
}

fn vorbis(bytes: &[u8]) -> Result<MetadataRevision> {
    // Callers bound the comment block size; catch_unwind contains reader panics.
    let mut builder = MetadataBuilder::new(
        symphonia::default::meta::embedded::vorbis::VORBIS_COMMENT_METADATA_INFO,
    );
    let mut reader = symphonia::core::io::BufReader::new(bytes);
    let mut side_data = Vec::new();
    catch_unwind(AssertUnwindSafe(|| {
        symphonia::default::meta::embedded::vorbis::read_vorbis_comment(
            &mut reader,
            &mut builder,
            &mut side_data,
        )
    }))
    .map_err(|_| "Vorbis-Tags konnten nicht sicher gelesen werden")?
    .map_err(|error| format!("Vorbis-Tags nicht lesbar: {error}"))?;
    Ok(builder.build())
}

fn flac(source: &mut TagSource<'_>) -> Result<EmbeddedAudioTags> {
    if &read_exact::<4>(source)? != b"fLaC" {
        return Err("FLAC-Tag-Container ist nicht lesbar".into());
    }
    let length = source
        .file
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    let mut tags = EmbeddedAudioTags::default();
    let mut entries = 0;
    let mut tag_bytes = 0usize;
    for index in 0..MAX_CHUNKS {
        let header = read_exact::<4>(source)?;
        let kind = header[0] & 0x7f;
        let size = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        let pos = source
            .stream_position()
            .map_err(|error| error.to_string())?;
        let next = pos
            .checked_add(size as u64)
            .filter(|next| *next <= length)
            .ok_or("FLAC-Tag-Block ist unvollständig")?;
        if index == 0 && (kind != 0 || size != 34) {
            return Err("FLAC-StreamInfo-Header ist nicht lesbar".into());
        }
        if kind == 127 {
            return Err("FLAC-Tag-Block hat einen ungültigen Typ".into());
        }
        if kind == 4 {
            tag_bytes = tag_bytes
                .checked_add(size)
                .filter(|size| *size <= MAX_TAG_BYTES)
                .ok_or("FLAC-Tags überschreiten 256 KiB Quelldaten")?;
            tags.add(&vorbis(&body(source, size)?)?, None, &mut entries)?;
        }
        // Pictures/application/audio are skipped, never decoded or copied for this text inspector.
        source
            .seek(SeekFrom::Start(next))
            .map_err(|error| error.to_string())?;
        if header[0] & 0x80 != 0 {
            return Ok(tags);
        }
    }
    Err("FLAC-Tags überschreiten 4096 Container-Einträge".into())
}

fn ogg(source: &mut TagSource<'_>) -> Result<EmbeddedAudioTags> {
    let mut serial = None;
    let mut sequence = 0u32;
    let mut packet = Vec::new();
    let mut identified = false;
    for index in 0..MAX_CHUNKS {
        let mut header = read_exact::<27>(source)?;
        if &header[..4] != b"OggS" || header[4] != 0 || header[5] & !7 != 0 {
            return Err("OGG-Tag-Seite ist nicht lesbar".into());
        }
        let page_serial = u32::from_le_bytes(header[14..18].try_into().unwrap());
        if serial.is_some_and(|serial| serial != page_serial) {
            return Ok(EmbeddedAudioTags::unsupported(
                "Tags in multiplexierten OGG-Streams werden noch nicht unterstützt",
            ));
        }
        serial = Some(page_serial);
        let page_sequence = u32::from_le_bytes(header[18..22].try_into().unwrap());
        if page_sequence != sequence
            || (index == 0 && header[5] & 2 == 0)
            || (index > 0 && header[5] & 2 != 0)
            || (header[5] & 1 != 0) != !packet.is_empty()
        {
            return Err("OGG-Tag-Sequenz ist unvollständig".into());
        }
        sequence = sequence.wrapping_add(1);
        let mut lacing = vec![0; header[26] as usize];
        source
            .read_exact(&mut lacing)
            .map_err(|error| format!("OGG-Tag-Segmente nicht lesbar: {error}"))?;
        let size: usize = lacing.iter().map(|size| *size as usize).sum();
        let page = body(source, size)?;
        let expected_crc = u32::from_le_bytes(header[22..26].try_into().unwrap());
        header[22..26].fill(0);
        let mut crc = Crc32::new(0);
        crc.process_buf_bytes(&header);
        crc.process_buf_bytes(&lacing);
        crc.process_buf_bytes(&page);
        if crc.crc() != expected_crc {
            return Err("OGG-Tag-Seite hat eine ungültige Prüfsumme".into());
        }
        let mut offset = 0;
        for size in lacing {
            let size = size as usize;
            if packet
                .len()
                .checked_add(size)
                .is_none_or(|size| size > MAX_TAG_BYTES)
            {
                return Err("OGG-Tag-Paket überschreitet 256 KiB Quelldaten".into());
            }
            packet.extend_from_slice(&page[offset..offset + size]);
            offset += size;
            if size == 255 {
                continue;
            }
            if !identified {
                if packet.len() != 30 || &packet[..7] != b"\x01vorbis" {
                    return Ok(EmbeddedAudioTags::unsupported(
                        "Eingebettete OGG-Tags werden derzeit nur für Vorbis unterstützt",
                    ));
                }
                if packet[7..11] != [0; 4] || packet[29] != 1 {
                    return Err("OGG-Vorbis-Header ist nicht lesbar".into());
                }
                identified = true;
                packet.clear();
            } else {
                if packet.len() < 8 || &packet[..7] != b"\x03vorbis" || packet.last() != Some(&1) {
                    return Err("OGG-Vorbis-Tag-Paket ist nicht lesbar".into());
                }
                let mut tags = EmbeddedAudioTags::default();
                tags.add(&vorbis(&packet[7..packet.len() - 1])?, None, &mut 0)?;
                return Ok(tags);
            }
        }
        if header[5] & 4 != 0 {
            return Err("OGG-Stream endet vor den eingebetteten Tags".into());
        }
    }
    Err("OGG-Tags überschreiten 4096 Container-Einträge".into())
}

fn syncsafe(bytes: &[u8]) -> Result<usize> {
    if bytes.len() != 4 || bytes.iter().any(|byte| byte & 0x80 != 0) {
        return Err("ID3-Tag hat eine ungültige Größenangabe".into());
    }
    Ok(bytes
        .iter()
        .fold(0, |value, byte| (value << 7) | *byte as usize))
}

fn id3_text(bytes: &[u8]) -> Result<()> {
    let (&encoding, bytes) = bytes.split_first().ok_or("ID3-Text-Tag ist leer")?;
    if bytes.len() > MAX_FIELD_BYTES {
        return Err("ID3-Text-Tag überschreitet 4 KiB Quelldaten".into());
    }
    match encoding {
        0 => Ok(()),
        3 => std::str::from_utf8(bytes)
            .map(|_| ())
            .map_err(|_| "ID3-Text-Tag enthält ungültigen UTF-8-Text".into()),
        1 | 2 => {
            let (bytes, little) = if encoding == 1 {
                match bytes.get(..2) {
                    Some([0xff, 0xfe]) => (&bytes[2..], true),
                    Some([0xfe, 0xff]) => (&bytes[2..], false),
                    _ => return Err("ID3-UTF-16-Tag hat keine gültige Byte-Reihenfolge".into()),
                }
            } else {
                (bytes, false)
            };
            if bytes.len() & 1 != 0 {
                return Err("ID3-UTF-16-Tag ist unvollständig".into());
            }
            let units = bytes.as_chunks::<2>().0.iter().map(|pair| {
                if little {
                    u16::from_le_bytes(*pair)
                } else {
                    u16::from_be_bytes(*pair)
                }
            });
            if char::decode_utf16(units).any(|value| value.is_err()) {
                return Err("ID3-UTF-16-Tag enthält ungültigen Text".into());
            }
            Ok(())
        }
        _ => Err("ID3-Text-Tag hat eine nicht unterstützte Zeichenkodierung".into()),
    }
}

fn mp3(source: &mut TagSource<'_>) -> Result<EmbeddedAudioTags> {
    const ID3V1_MESSAGE: &str =
        "Zusätzliche ID3v1-Tags am MP3-Dateiende werden noch nicht unterstützt";
    let length = source
        .file
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    let has_id3v1 = if length >= 128 {
        source
            .seek(SeekFrom::Start(length - 128))
            .map_err(|error| error.to_string())?;
        let marker = read_exact::<3>(source)?;
        source
            .seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        &marker == b"TAG"
    } else {
        false
    };
    let header = read_exact::<10>(source)?;
    if &header[..3] != b"ID3" {
        return Ok(if has_id3v1 {
            EmbeddedAudioTags::unsupported(ID3V1_MESSAGE)
        } else {
            EmbeddedAudioTags::default()
        });
    }
    if !matches!(header[3], 3 | 4) || header[4] != 0 || header[5] != 0 {
        return Ok(EmbeddedAudioTags::unsupported("Eingebettete MP3-Tags werden für einfache ID3v2.3/v2.4 unterstützt; andere Versionen oder Header-Flags sind noch nicht verfügbar"));
    }
    let data = body(source, syncsafe(&header[6..10])?)?;
    let mut pos = 0;
    let mut count = 0;
    while pos < data.len() {
        if data[pos] == 0 {
            if data[pos..].iter().any(|byte| *byte != 0) {
                return Err("ID3-Tag enthält ungültige Padding-Daten".into());
            }
            break;
        }
        count += 1;
        if count > MAX_ENTRIES {
            return Err("ID3-Tags überschreiten 256 Einträge".into());
        }
        let frame = take(&data, &mut pos, 10)?;
        if frame[..4]
            .iter()
            .any(|byte| !byte.is_ascii_uppercase() && !byte.is_ascii_digit())
        {
            return Err("ID3-Tag hat einen ungültigen Feldnamen".into());
        }
        if frame[8..10] != [0, 0] || &frame[..4] == b"CHAP" || &frame[..4] == b"CTOC" {
            return Ok(EmbeddedAudioTags::unsupported(
                "ID3-Tags mit Frame-Flags oder Kapitelstrukturen werden noch nicht unterstützt",
            ));
        }
        let size = if header[3] == 4 {
            syncsafe(&frame[4..8])?
        } else {
            u32::from_be_bytes(frame[4..8].try_into().unwrap()) as usize
        };
        if size > MAX_FIELD_BYTES {
            return Err("ID3-Tag überschreitet 4 KiB Quelldaten".into());
        }
        let value = take(&data, &mut pos, size)?;
        if matches!(&frame[..4], b"TIT2" | b"TPE1" | b"TALB" | b"TCON") {
            id3_text(value)?;
        }
    }
    // ponytail: preflight plain text envelopes because Symphonia 0.6.1 ignores metadata limits and suppresses malformed frame errors.
    let mut input = header.to_vec();
    input.extend_from_slice(&data);
    let stream = MediaSourceStream::new(Box::new(Cursor::new(input)), Default::default());
    let revision = catch_unwind(AssertUnwindSafe(|| {
        symphonia::default::meta::Id3v2Reader::try_new(stream, Default::default())
            .and_then(|mut reader| reader.read_all())
    }))
    .map_err(|_| "ID3-Tags konnten nicht sicher gelesen werden")?
    .map_err(|error| format!("ID3-Tags nicht lesbar: {error}"))?;
    let mut tags = EmbeddedAudioTags::default();
    tags.add(&revision.revision, None, &mut 0)?;
    if has_id3v1 {
        tags.status = AudioTagStatus::Unsupported;
        tags.message = Some(ID3V1_MESSAGE.into());
    }
    Ok(tags)
}

#[cfg(test)]
#[path = "audio_tags_tests.rs"]
mod tests;
