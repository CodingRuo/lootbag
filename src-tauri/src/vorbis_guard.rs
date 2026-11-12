use super::Result;
use std::io;
use symphonia::core::{
    codecs::audio::{well_known::CODEC_ID_VORBIS, AudioCodecParameters},
    io::{BitReaderRtl, ReadBitsRtl},
};

const MAX_SETUP_BYTES: usize = 1024 * 1024;
const MAX_SETUP_ALLOCATION: u64 = 128 * 1024 * 1024;

// ponytail: this budget follows Symphonia 0.6.1's codebook layout; recheck on decoder upgrades.

pub(super) fn validate(parameters: &AudioCodecParameters) -> Result<()> {
    if parameters.codec != CODEC_ID_VORBIS {
        return Ok(());
    }
    let data = parameters
        .extra_data
        .as_deref()
        .ok_or("Vorbis-Setup fehlt")?;
    if data.len() > MAX_SETUP_BYTES {
        return Err("Vorbis-Setup überschreitet die Header-Grenze von 1 MiB".into());
    }
    // Ogg's installed mapper supplies a fixed identification packet followed by raw setup.
    if data.len() < 38 || &data[..7] != b"\x01vorbis" || &data[30..37] != b"\x05vorbis" {
        return Err("Vorbis-Setup ist unvollständig oder hat ein unbekanntes Header-Layout".into());
    }
    let rate = u32::from_le_bytes(data[12..16].try_into().unwrap());
    let small = data[28] & 15;
    let large = data[28] >> 4;
    if !(1..=8).contains(&data[11])
        || rate == 0
        || rate > 192_000
        || !(6..=13).contains(&small)
        || !(small..=13).contains(&large)
    {
        return Err("Vorbis-Header überschreitet die Audioanalyse-Grenzen".into());
    }
    codebooks(&mut BitReaderRtl::new(&data[37..]))
        .map_err(|error| format!("Vorbis-Setup nicht lesbar: {error}"))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn codebooks(bits: &mut BitReaderRtl<'_>) -> io::Result<()> {
    let count = bits.read_bits_leq32(8)? + 1;
    let mut allocation = 0u64;
    for _ in 0..count {
        if bits.read_bits_leq32(24)? != 0x564342 {
            return Err(invalid("Ungültiger Codebook-Marker"));
        }
        let dimensions = bits.read_bits_leq32(16)?;
        let entries = bits.read_bits_leq32(24)?;
        if !(1..=32).contains(&dimensions) || !(1..=131_072).contains(&entries) {
            return Err(invalid("Vorbis-Setup überschreitet das Analysebudget"));
        }
        // Symphonia 0.6.1 expands VQ and builds complete 32-bit Huffman trees. Charge
        // conservative scalar/LUT capacity plus both VQ floats and multiplicands.
        allocation += u64::from(entries) * (2048 + u64::from(dimensions) * 8);
        if allocation > MAX_SETUP_ALLOCATION {
            return Err(invalid(
                "Vorbis-Setup überschreitet das Analysebudget von 128 MiB",
            ));
        }
        if bits.read_bool()? {
            let mut length = bits.read_bits_leq32(5)? + 1;
            let mut remaining = entries;
            while remaining > 0 {
                if length > 32 {
                    return Err(invalid("Vorbis-Codewort überschreitet 32 Bit"));
                }
                let width = 32 - remaining.leading_zeros();
                let run = bits.read_bits_leq32(width)?;
                if run > remaining {
                    return Err(invalid("Ungültige Vorbis-Codewort-Anzahl"));
                }
                remaining -= run;
                length += 1;
            }
        } else if bits.read_bool()? {
            for _ in 0..entries {
                if bits.read_bool()? {
                    bits.ignore_bits(5)?;
                }
            }
        } else {
            bits.ignore_bits(entries * 5)?;
        }
        match bits.read_bits_leq32(4)? {
            0 => (),
            lookup @ (1 | 2) => {
                bits.ignore_bits(64)?;
                let value_bits = bits.read_bits_leq32(4)? + 1;
                bits.ignore_bit()?;
                let values = if lookup == 1 {
                    let root = (entries as f32).powf(1.0 / dimensions as f32).floor() as u32;
                    // Match the installed decoder's lookup count without permitting its
                    // unchecked integer pow assertion to overflow on malformed headers.
                    if root == 0
                        || !root
                            .checked_pow(dimensions)
                            .is_some_and(|value| value <= entries)
                    {
                        return Err(invalid("Ungültige Vorbis-Lookup-Größe"));
                    }
                    root
                } else {
                    entries * dimensions
                };
                bits.ignore_bits(values * value_bits)?;
            }
            _ => return Err(invalid("Ungültiger Vorbis-Lookup-Typ")),
        }
    }
    // Decoder validates the rest. Floor/residue/mapping counts are six-bit bounded;
    // their allocations and DSP buffers are bounded by the identification block size.
    Ok(())
}
