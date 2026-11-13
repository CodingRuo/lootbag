//! Licence suggestions from plain-text licence/readme files: bounded reads, no links followed,
//! nothing rendered or fetched. Only explicit names with version or canonical licence URLs count.
use super::super::imports::{checked_zip_path, open_zip};
use super::{is_license_name, sorted_hints, Result, MAX_SCAN_ENTRIES};
use std::{fs, io::Read, path::Path};

/// Bytes read per file; licence names and URLs sit near the top.
const MAX_TEXT_BYTES: u64 = 64 << 10;

/// Package-relative path and text of each file read.
pub(super) type Texts = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseSuggestion {
    pub license: String,
    /// Package-relative file the evidence comes from.
    pub file: String,
    /// The matching line (shortened), shown as plain text.
    pub excerpt: String,
}

/// One suggestion per recognized licence, from the first file (in hint order) that names it.
pub(super) fn suggestions(texts: &[(String, String)]) -> Vec<LicenseSuggestion> {
    let mut found: Vec<LicenseSuggestion> = Vec::new();
    for (file, text) in texts {
        for (license, excerpt) in recognize(text) {
            if !found.iter().any(|known| known.license == license) {
                found.push(LicenseSuggestion {
                    license: license.into(),
                    file: file.clone(),
                    excerpt,
                });
            }
        }
    }
    found
}

/// Licences named in `text`, each with the excerpt of its first mention, in text order.
fn recognize(text: &str) -> Vec<(&'static str, String)> {
    // ASCII-lowercased with whitespace, '-' and '_' runs as one space; `origin` maps back to `text`.
    let mut flat = String::new();
    let mut origin = Vec::new();
    for (at, c) in text.char_indices() {
        let c = if c.is_whitespace() || c == '-' || c == '_' {
            ' '
        } else {
            c.to_ascii_lowercase()
        };
        if c == ' ' && flat.ends_with(' ') {
            continue;
        }
        let mut buffer = [0; 4];
        for _ in c.encode_utf8(&mut buffer).bytes() {
            origin.push(at);
        }
        flat.push(c);
    }
    let mut patterns: Vec<(&'static str, String)> = [
        ("CC0 1.0", "cc0"),
        ("CC0 1.0", "creative commons zero"),
        ("CC0 1.0", "creativecommons.org/publicdomain/zero/1.0"),
        ("MIT", "mit license"),
        ("MIT", "mit licence"),
        ("MIT", "opensource.org/licenses/mit"),
    ]
    .map(|(license, needle)| (license, needle.to_owned()))
    .into();
    for (version, by, by_sa) in [
        ("3.0", "CC BY 3.0", "CC BY-SA 3.0"),
        ("4.0", "CC BY 4.0", "CC BY-SA 4.0"),
    ] {
        for needle in [
            "cc by",
            "creative commons attribution",
            "attribution {} international",
            "creativecommons.org/licenses/by/",
        ] {
            patterns.push((by, versioned(needle, version)));
        }
        for needle in [
            "cc by sa",
            "creative commons attribution sharealike",
            "creative commons attribution share alike",
            "attribution sharealike {} international",
            "creativecommons.org/licenses/by sa/",
        ] {
            patterns.push((by_sa, versioned(needle, version)));
        }
    }
    let mut hits: Vec<(usize, &'static str, String)> = Vec::new();
    for (license, needle) in &patterns {
        let Some(start) = find_word(&flat, needle) else {
            continue;
        };
        let end = origin[start + needle.len() - 1] + 1;
        match hits.iter_mut().find(|hit| hit.1 == *license) {
            Some(hit) if hit.0 <= start => {}
            Some(hit) => *hit = (start, license, excerpt(text, origin[start], end)),
            None => hits.push((start, license, excerpt(text, origin[start], end))),
        }
    }
    hits.sort_by_key(|hit| hit.0);
    hits.into_iter()
        .map(|(_, license, excerpt)| (license, excerpt))
        .collect()
}

/// `needle` with the version appended, or put in place of `{}`.
fn versioned(needle: &str, version: &str) -> String {
    if needle.contains("{}") {
        needle.replace("{}", version)
    } else if needle.ends_with('/') {
        format!("{needle}{version}")
    } else {
        format!("{needle} {version}")
    }
}

/// First occurrence of `needle` not glued to letters or digits on either side.
fn find_word(haystack: &str, needle: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let glued = |at: Option<&u8>| at.is_some_and(u8::is_ascii_alphanumeric);
    haystack
        .match_indices(needle)
        .map(|(at, _)| at)
        .find(|&at| {
            !glued(at.checked_sub(1).and_then(|before| bytes.get(before)))
                && !glued(bytes.get(at + needle.len()))
        })
}

/// The line around `start..end`, cut to about 60 bytes on each side and with whitespace collapsed.
fn excerpt(text: &str, start: usize, end: usize) -> String {
    let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |at| end + at);
    let mut from = line_start.max(start.saturating_sub(60));
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let mut to = line_end.min(end + 60);
    while !text.is_char_boundary(to) {
        to -= 1;
    }
    let body = text[from..to]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cut = |cut: bool| if cut { "…" } else { "" };
    format!("{}{body}{}", cut(from > line_start), cut(to < line_end))
}

/// Whether a hint file is read: plain text only (PDFs, HTML and the like stay hints).
fn is_plain_text(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| ext.eq_ignore_ascii_case("txt") || ext.eq_ignore_ascii_case("md"))
}

/// Text of NUL-free bytes; anything binary is skipped.
fn text(bytes: Vec<u8>) -> Option<String> {
    (!bytes.contains(&0)).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// Reads plain-text `files` (package-relative, from the hint scan) below `base` without
/// following links: every path component is checked again right before reading.
pub(super) fn folder_texts(base: &Path, files: &[String]) -> Texts {
    files
        .iter()
        .filter(|file| is_plain_text(file))
        .filter_map(|file| {
            let mut path = base.to_owned();
            for part in file.split('/') {
                path.push(part);
                if fs::symlink_metadata(&path).ok()?.file_type().is_symlink() {
                    return None;
                }
            }
            if !fs::symlink_metadata(&path).ok()?.is_file() {
                return None;
            }
            let mut bytes = Vec::new();
            fs::File::open(&path)
                .ok()?
                .take(MAX_TEXT_BYTES)
                .read_to_end(&mut bytes)
                .ok()?;
            Some((file.clone(), text(bytes)?))
        })
        .collect()
}

/// Licence/readme hints at the archive root and one level below, plus the texts of the plain ones.
pub(super) fn zip_hints(archive: &Path) -> Result<(Vec<String>, Texts)> {
    let mut zip = open_zip(archive)?;
    let mut found = Vec::new();
    for index in 0..zip.len().min(MAX_SCAN_ENTRIES) {
        let Ok(entry) = zip.by_index_raw(index) else {
            continue;
        };
        // Unsafe names, links and special files are skipped like on import.
        if entry.is_dir() || entry.encrypted() || checked_zip_path(&entry).is_err() {
            continue;
        }
        // Checked names use '/' only and have no empty, '.' or '..' parts.
        let parts: Vec<_> = entry.name().split('/').collect();
        if parts.len() <= 2 && parts[0] != "__MACOSX" && is_license_name(parts[parts.len() - 1]) {
            found.push(entry.name().to_owned());
        }
    }
    let files = sorted_hints(found);
    let texts = files
        .iter()
        .filter(|file| is_plain_text(file))
        .filter_map(|file| {
            let entry = zip.by_name(file).ok()?;
            let mut bytes = Vec::new();
            entry.take(MAX_TEXT_BYTES).read_to_end(&mut bytes).ok()?;
            Some((file.clone(), text(bytes)?))
        })
        .collect();
    Ok((files, texts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<&'static str> {
        recognize(text)
            .into_iter()
            .map(|(license, _)| license)
            .collect()
    }

    #[test]
    fn each_licence_is_recognized_by_name_version_or_url() {
        for (text, license) in [
            ("License: (Creative Commons Zero, CC0)", "CC0 1.0"),
            (
                "http://creativecommons.org/publicdomain/zero/1.0/",
                "CC0 1.0",
            ),
            ("Released under CC0-1.0.", "CC0 1.0"),
            ("CC-BY 4.0 – credit me", "CC BY 4.0"),
            ("Creative Commons Attribution 3.0 Unported", "CC BY 3.0"),
            ("Attribution 4.0 International", "CC BY 4.0"),
            ("https://creativecommons.org/licenses/by/3.0/", "CC BY 3.0"),
            ("cc_by_4.0", "CC BY 4.0"),
            ("CC BY-SA 4.0", "CC BY-SA 4.0"),
            (
                "Creative Commons Attribution-ShareAlike 3.0",
                "CC BY-SA 3.0",
            ),
            (
                "https://creativecommons.org/licenses/by-sa/4.0/",
                "CC BY-SA 4.0",
            ),
            ("The MIT License (MIT)\n\nCopyright (c) 2024", "MIT"),
            ("see https://opensource.org/licenses/MIT", "MIT"),
        ] {
            assert_eq!(names(text), [license], "{text}");
        }
    }

    #[test]
    fn vague_versionless_and_other_licences_are_not_guessed() {
        for text in [
            "All assets are royalty free for commercial use.",
            "Made by Kenney (www.kenney.nl)",
            "Creative Commons Attribution",
            "CC-BY",
            "Creative Commons Attribution-NonCommercial 4.0",
            "https://creativecommons.org/licenses/by-nc/4.0/",
            "Attribution-NonCommercial-ShareAlike 4.0 International",
            "Please submit license questions by mail.",
            "MIT",
            "CC0x",
        ] {
            assert!(names(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn conflicting_licences_are_listed_with_their_own_evidence() {
        let text = "Sprites: CC BY 4.0\nSounds: CC0\nCode: MIT License\nMore sprites: CC-BY 4.0";
        assert_eq!(
            recognize(text),
            [
                ("CC BY 4.0", "Sprites: CC BY 4.0".to_owned()),
                ("CC0 1.0", "Sounds: CC0".to_owned()),
                ("MIT", "Code: MIT License".to_owned()),
            ]
        );
        let texts = [
            ("License.txt".to_owned(), "CC0".to_owned()),
            ("Readme.md".to_owned(), "CC0 or MIT License".to_owned()),
        ];
        let found = suggestions(&texts);
        assert_eq!(
            found
                .iter()
                .map(|s| (s.license.as_str(), s.file.as_str()))
                .collect::<Vec<_>>(),
            [("CC0 1.0", "License.txt"), ("MIT", "Readme.md")]
        );
    }

    #[test]
    fn excerpts_are_short_and_stay_on_char_boundaries() {
        let long = format!("{} CC0 {}", "ä".repeat(100), "ö".repeat(100));
        let (_, excerpt) = &recognize(&long)[0];
        assert!(excerpt.starts_with('…') && excerpt.ends_with('…'));
        assert!(excerpt.contains("CC0") && excerpt.len() < 140);
        assert!(is_plain_text("LICENSE") && is_plain_text("Readme.MD"));
        assert!(!is_plain_text("License.pdf") && !is_plain_text("README.html"));
        assert_eq!(text(b"CC0\0".to_vec()), None);
    }
}
