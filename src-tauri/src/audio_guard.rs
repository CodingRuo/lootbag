use super::super::Result;
use std::io::{Read, Seek, SeekFrom};
use symphonia::core::{checksum::Crc32, io::Monitor};

const MAX_CHUNKS: usize = 4096;
const MAX_METADATA: usize = 1024 * 1024;
const MAX_FIELD: usize = 4096;
const MAX_COMMENT: usize = 256 * 1024;

fn read<const N: usize>(source: &mut (impl Read + Seek)) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    source
        .read_exact(&mut bytes)
        .map_err(|e| format!("Audioheader unvollständig: {e}"))?;
    Ok(bytes)
}
fn position(source: &mut (impl Read + Seek)) -> Result<u64> {
    source.stream_position().map_err(|e| e.to_string())
}
fn seek(source: &mut (impl Read + Seek), end: u64) -> Result<()> {
    source
        .seek(SeekFrom::Start(end))
        .map(|_| ())
        .map_err(|e| e.to_string())
}
fn end(source: &mut (impl Read + Seek), size: u64, limit: u64) -> Result<u64> {
    position(source)?
        .checked_add(size)
        .filter(|v| *v <= limit)
        .ok_or("Audioheader überschreitet seine Container-Grenze".into())
}
fn bytes(source: &mut (impl Read + Seek), size: usize) -> Result<Vec<u8>> {
    if size > MAX_COMMENT {
        return Err("Audio-Metadatenblock überschreitet 256 KiB".into());
    }
    let mut data = vec![0; size];
    source.read_exact(&mut data).map_err(|e| e.to_string())?;
    Ok(data)
}
fn take<'a>(data: &'a [u8], pos: &mut usize, count: usize) -> Result<&'a [u8]> {
    let next = pos
        .checked_add(count)
        .filter(|v| *v <= data.len())
        .ok_or("Audio-Metadaten sind unvollständig")?;
    let value = &data[*pos..next];
    *pos = next;
    Ok(value)
}
fn number(data: &[u8], pos: &mut usize, little: bool) -> Result<usize> {
    let value = take(data, pos, 4)?.try_into().unwrap();
    Ok(if little {
        u32::from_le_bytes(value)
    } else {
        u32::from_be_bytes(value)
    } as usize)
}

pub(in crate::library) fn comments(data: &[u8], entries: &mut usize) -> Result<()> {
    let mut pos = 0;
    let vendor = number(data, &mut pos, true)?;
    if vendor > MAX_FIELD {
        return Err("Vorbis-Vendor überschreitet 4 KiB Text".into());
    }
    std::str::from_utf8(take(data, &mut pos, vendor)?)
        .map_err(|_| "Vorbis-Vendor enthält ungültigen UTF-8-Text")?;
    let count = number(data, &mut pos, true)?;
    *entries = entries
        .checked_add(count)
        .filter(|v| *v <= 256)
        .ok_or("Vorbis-Tags überschreiten insgesamt 256 Einträge")?;
    for _ in 0..count {
        let size = number(data, &mut pos, true)?;
        if size > MAX_FIELD {
            return Err("Vorbis-Tag überschreitet 4 KiB Quelldaten".into());
        }
        let field = take(data, &mut pos, size)?;
        std::str::from_utf8(field).map_err(|_| "Vorbis-Tag enthält ungültigen UTF-8-Text")?;
        let key = field
            .iter()
            .position(|b| *b == b'=')
            .ok_or("Vorbis-Tag hat keinen Feldnamen oder Wert")?;
        if key == 0 || field[..key].iter().any(|b| !(0x20..=0x7d).contains(b)) {
            return Err("Vorbis-Tag hat einen ungültigen Feldnamen".into());
        }
    }
    if pos != data.len() {
        return Err("Vorbis-Tag enthält unerwartete Restdaten".into());
    }
    Ok(())
}

pub(super) fn validate(
    source: &mut (impl Read + Seek),
    length: u64,
    signature: &[u8; 4],
) -> Result<()> {
    match signature {
        b"RIFF" => wav(source, length),
        b"fLaC" => flac(source, length),
        b"OggS" => ogg(source, length),
        _ => Ok(()),
    }
}
fn wav(source: &mut (impl Read + Seek), length: u64) -> Result<()> {
    let h = read::<12>(source)?;
    if &h[8..] != b"WAVE" {
        return Err("RIFF ist kein WAV-Container".into());
    }
    let declared = u32::from_le_bytes(h[4..8].try_into().unwrap());
    let limit = if declared == u32::MAX {
        length
    } else {
        8 + u64::from(declared)
    };
    if limit < 12 || limit > length {
        return Err("WAV-Container ist unvollständig".into());
    }
    let mut entries = 0;
    let mut metadata = 0;
    for _ in 0..MAX_CHUNKS {
        if position(source)? == limit {
            return Ok(());
        }
        let h = read::<8>(source)?;
        let count = u32::from_le_bytes(h[4..8].try_into().unwrap());
        if &h[..4] == b"data" && count == u32::MAX {
            return Ok(());
        }
        let next = end(source, u64::from(count) + u64::from(count & 1), limit)?;
        if &h[..4] == b"LIST" {
            if count < 4 {
                return Err("WAV-LIST ist unvollständig".into());
            }
            if &read::<4>(source)? == b"INFO" {
                let list_end = next - u64::from(count & 1);
                while position(source)? < list_end {
                    let field = read::<8>(source)?;
                    let size = u32::from_le_bytes(field[4..8].try_into().unwrap()) as usize;
                    let following = end(source, (size + (size & 1)) as u64, list_end)?;
                    entries += 1;
                    metadata += size;
                    if entries > 256 || size > MAX_FIELD || metadata > MAX_COMMENT {
                        return Err("WAV-INFO überschreitet die Metadaten-Grenze".into());
                    }
                    seek(source, following)?;
                }
            }
        }
        seek(source, next)?;
    }
    Err("WAV überschreitet 4096 Container-Einträge".into())
}
fn picture(data: &[u8]) -> Result<()> {
    let mut pos = 4;
    for _ in 0..2 {
        let size = number(data, &mut pos, false)?;
        if size > MAX_FIELD {
            return Err("FLAC-Bildtext überschreitet 4 KiB".into());
        }
        take(data, &mut pos, size)?;
    }
    take(data, &mut pos, 16)?;
    let size = number(data, &mut pos, false)?;
    take(data, &mut pos, size)?;
    if pos != data.len() {
        return Err("FLAC-Bildblock hat ungültige Längen".into());
    }
    Ok(())
}
fn cuesheet(data: &[u8], entries: &mut usize) -> Result<()> {
    let mut pos = 395;
    let tracks = take(data, &mut pos, 1)?[0];
    for _ in 0..tracks {
        take(data, &mut pos, 35)?;
        let indices = take(data, &mut pos, 1)?[0] as usize;
        *entries += 1 + indices;
        if *entries > 256 {
            return Err("FLAC-Cuesheet überschreitet 256 Einträge".into());
        }
        take(data, &mut pos, indices * 12)?;
    }
    if pos != data.len() {
        return Err("FLAC-Cuesheet ist unvollständig".into());
    }
    Ok(())
}
fn flac(source: &mut (impl Read + Seek), length: u64) -> Result<()> {
    read::<4>(source)?;
    let mut metadata = 0usize;
    let mut entries = 0usize;
    let mut cue_entries = 0usize;
    for index in 0..MAX_CHUNKS {
        let h = read::<4>(source)?;
        let kind = h[0] & 0x7f;
        let size = u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize;
        let next = end(source, size as u64, length)?;
        if (index == 0 && (kind != 0 || size != 34)) || kind == 127 {
            return Err("FLAC-Metadatenheader ungültig".into());
        }
        if kind != 1 {
            metadata += size;
            if metadata > MAX_METADATA || size > MAX_COMMENT {
                return Err("FLAC-Metadaten überschreiten die Analysegrenze".into());
            }
        }
        match kind {
            2 if size < 4 => return Err("FLAC-Anwendungsblock ist unvollständig".into()),
            3 if size % 18 != 0 || size / 18 > 4096 => {
                return Err(
                    "FLAC-Seektable überschreitet 4096 Einträge oder ist unvollständig".into(),
                )
            }
            4 => comments(&bytes(source, size)?, &mut entries)?,
            5 => cuesheet(&bytes(source, size)?, &mut cue_entries)?,
            6 => picture(&bytes(source, size)?)?,
            _ => (),
        }
        seek(source, next)?;
        if h[0] & 0x80 != 0 {
            return Ok(());
        }
    }
    Err("FLAC überschreitet 4096 Metadatenblöcke".into())
}
fn ogg(source: &mut (impl Read + Seek), length: u64) -> Result<()> {
    let mut serial = None;
    let mut pending = 0usize;
    let mut packet = Vec::new();
    let mut headers = 0;
    // ponytail: qualify one nonchained Vorbis stream; multiplex/chained sources need explicit per-stream budgets.
    for page_index in 0..100_000u32 {
        let mut h = read::<27>(source)?;
        if &h[..4] != b"OggS" || h[4] != 0 || h[5] & !7 != 0 {
            return Err("OGG-Seitenheader ungültig".into());
        }
        let id = u32::from_le_bytes(h[14..18].try_into().unwrap());
        if serial.is_some_and(|prior| prior != id) {
            return Err("Multiplexierte oder verkettete OGG-Streams werden für die Audioanalyse noch nicht unterstützt".into());
        }
        serial = Some(id);
        if u32::from_le_bytes(h[18..22].try_into().unwrap()) != page_index
            || (page_index == 0) != (h[5] & 2 != 0)
            || (pending != 0) != (h[5] & 1 != 0)
        {
            return Err("OGG-Sequenz ist unvollständig".into());
        }
        let mut lacing = vec![0; h[26] as usize];
        source.read_exact(&mut lacing).map_err(|e| e.to_string())?;
        let size: usize = lacing.iter().map(|v| *v as usize).sum();
        let payload_start = position(source)?;
        let next = end(source, size as u64, length)?;
        let payload = if headers < 3 {
            bytes(source, size)?
        } else {
            Vec::new()
        };
        if headers < 3 {
            let expected = u32::from_le_bytes(h[22..26].try_into().unwrap());
            h[22..26].fill(0);
            let mut crc = Crc32::new(0);
            crc.process_buf_bytes(&h);
            crc.process_buf_bytes(&lacing);
            crc.process_buf_bytes(&payload);
            if crc.crc() != expected {
                return Err("OGG-Header-Prüfsumme ungültig".into());
            }
        }
        let mut offset = 0;
        for segment in lacing {
            let size = segment as usize;
            if headers == 3 && pending == 0 && size > 0 {
                let first = if payload.is_empty() {
                    seek(source, payload_start + offset as u64)?;
                    read::<1>(source)?[0]
                } else {
                    payload[offset]
                };
                if first & 1 != 0 {
                    return Err("OGG enthält einen unerwarteten weiteren Vorbis-Header".into());
                }
            }
            pending += size;
            if pending > 1024 * 1024 {
                return Err("OGG-Paket überschreitet 1 MiB".into());
            }
            if headers < 3 {
                if packet.len() + size > MAX_COMMENT {
                    return Err("OGG-Header überschreitet 256 KiB".into());
                }
                packet.extend_from_slice(&payload[offset..offset + size]);
            }
            offset += size;
            if segment == 255 {
                continue;
            }
            pending = 0;
            if headers < 3 {
                match headers {
                    0 if packet.len() != 30 || &packet[..7] != b"\x01vorbis" => {
                        return Err("OGG-Audiospur ist kein unterstützter Vorbis-Stream".into())
                    }
                    1 => {
                        if packet.len() < 8
                            || &packet[..7] != b"\x03vorbis"
                            || packet.last() != Some(&1)
                        {
                            return Err("OGG-Kommentarheader ungültig".into());
                        }
                        comments(&packet[7..packet.len() - 1], &mut 0)?;
                    }
                    2 if packet.len() < 7 || &packet[..7] != b"\x05vorbis" => {
                        return Err("OGG-Setupheader ungültig".into())
                    }
                    _ => (),
                }
                headers += 1;
                packet.clear();
            }
        }
        seek(source, next)?;
        if h[5] & 4 != 0 {
            if next != length || pending != 0 || headers != 3 {
                return Err("OGG-Streamende ist unvollständig".into());
            }
            return Ok(());
        }
        if next == length {
            return Err("OGG-Stream hat keine vollständige Endseite".into());
        }
    }
    Err("OGG überschreitet 100.000 Seiten".into())
}
