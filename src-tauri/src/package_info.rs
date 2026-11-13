use super::{file_identity, zip_sources, Library, Result};
use rusqlite::{params, OptionalExtension};
use std::{fs, path::Path};
#[path = "license_detection.rs"]
mod license_detection;
pub use license_detection::LicenseSuggestion;

/// User-entered record of where a package came from and under which licence; every field optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PackageInfo {
    pub store: Option<String>,
    pub url: Option<String>,
    pub license: Option<String>,
    pub author: Option<String>,
    /// ISO date `YYYY-MM-DD`.
    pub purchased: Option<String>,
    pub version: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInfoDetails {
    #[serde(flatten)]
    pub info: PackageInfo,
    /// Licence/readme files near the package root (relative paths); empty if the source is unavailable.
    pub license_files: Vec<String>,
    /// Licences named in the plain-text ones; several entries mean the package names several.
    pub license_suggestions: Vec<LicenseSuggestion>,
}

const MAX_SCAN_ENTRIES: usize = 2_000;
const MAX_HINTS: usize = 20;

impl Library {
    pub(crate) fn package_info(&self, package_id: i64) -> Result<PackageInfo> {
        self.connection
            .query_row(
                "SELECT store, url, license, author, purchased, version, notes FROM package_info WHERE package_id = ?1",
                [package_id],
                |row| {
                    Ok(PackageInfo {
                        store: row.get(0)?,
                        url: row.get(1)?,
                        license: row.get(2)?,
                        author: row.get(3)?,
                        purchased: row.get(4)?,
                        version: row.get(5)?,
                        notes: row.get(6)?,
                    })
                },
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(|e| format!("Paketinfo nicht lesbar: {e}"))
    }

    pub fn get_package_info(&self, package_id: i64) -> Result<PackageInfoDetails> {
        let source: Option<(String, Option<String>, Option<String>)> = self
            .connection
            .query_row(
                "SELECT p.relative_path, s.root, s.identity FROM active_packages p
                    LEFT JOIN external_sources s ON s.package_id = p.id WHERE p.id = ?1",
                [package_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("Paket nicht lesbar: {e}"))?;
        let folder = |base: &Path| {
            let files = license_files(base);
            let texts = license_detection::folder_texts(base, &files);
            (files, texts)
        };
        // External roots are read only while they are still the catalogued folder or archive.
        let (license_files, texts) = match source {
            Some((relative, None, _)) => folder(&self.root.join(relative)),
            Some((_, Some(root), Some(identity))) if zip_sources::is_zip(&identity) => {
                let root = Path::new(&root);
                if zip_sources::archive_identity(root).ok() == Some(identity) {
                    license_detection::zip_hints(root).unwrap_or_default()
                } else {
                    Default::default()
                }
            }
            Some((_, Some(root), Some(identity))) => {
                let root = Path::new(&root);
                let verified = fs::symlink_metadata(root).is_ok_and(|metadata| {
                    metadata.is_dir()
                        && !metadata.file_type().is_symlink()
                        && file_identity(root, &metadata).ok() == Some(identity)
                });
                if verified {
                    folder(root)
                } else {
                    Default::default()
                }
            }
            _ => Default::default(),
        };
        Ok(PackageInfoDetails {
            info: self.package_info(package_id)?,
            license_suggestions: license_detection::suggestions(&texts),
            license_files,
        })
    }

    pub fn set_package_info(&mut self, package_id: i64, info: &PackageInfo) -> Result<()> {
        let info = validated(info)?;
        let exists: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM active_packages WHERE id = ?1)",
                [package_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Paket nicht lesbar: {e}"))?;
        if !exists {
            return Err("Paket nicht im Katalog".into());
        }
        let result = if info == PackageInfo::default() {
            self.connection.execute(
                "DELETE FROM package_info WHERE package_id = ?1",
                [package_id],
            )
        } else {
            self.connection.execute(
                "INSERT OR REPLACE INTO package_info(package_id, store, url, license, author, purchased, version, notes)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![package_id, info.store, info.url, info.license, info.author, info.purchased, info.version, info.notes],
            )
        };
        result
            .map(drop)
            .map_err(|e| format!("Paketinfo kann nicht gespeichert werden: {e}"))
    }
}

/// Trimmed copy with empty fields as `None`; rejects overlong text, non-http links and invalid dates.
fn validated(info: &PackageInfo) -> Result<PackageInfo> {
    let field = |value: &Option<String>,
                 label: &str,
                 max: usize,
                 multiline: bool|
     -> Result<Option<String>> {
        let Some(value) = value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        if value.chars().count() > max {
            return Err(format!("{label} darf höchstens {max} Zeichen enthalten"));
        }
        if value
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\t')))
        {
            return Err(format!("{label} enthält unzulässige Steuerzeichen"));
        }
        Ok(Some(value.to_owned()))
    };
    let checked = PackageInfo {
        store: field(&info.store, "Shop", 200, false)?,
        url: field(&info.url, "Link", 2048, false)?,
        license: field(&info.license, "Lizenz", 200, false)?,
        author: field(&info.author, "Urheber", 200, false)?,
        purchased: field(&info.purchased, "Kaufdatum", 10, false)?,
        version: field(&info.version, "Version", 200, false)?,
        notes: field(&info.notes, "Notizen", 2000, true)?,
    };
    if let Some(url) = &checked.url {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) || url.contains(' ') {
            return Err("Der Link muss mit http:// oder https:// beginnen".into());
        }
    }
    if let Some(date) = &checked.purchased {
        if !valid_date(date) {
            return Err("Kaufdatum muss ein gültiges Datum im Format JJJJ-MM-TT sein".into());
        }
    }
    Ok(checked)
}

fn valid_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        bytes[range.clone()]
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| date[range].parse::<u32>().ok())
            .flatten()
    };
    let (Some(year), Some(month), Some(day)) = (number(0..4), number(5..7), number(8..10)) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    year > 0 && (1..=days).contains(&day)
}

/// LICENSE*/Licence*, *license*.txt|md|pdf and README* files in the package root and its direct subfolders.
/// Bounded and never follows links.
fn license_files(package: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut scanned = 0;
    let mut folders = vec![(package.to_path_buf(), String::new())];
    while let Some((folder, prefix)) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            scanned += 1;
            if scanned > MAX_SCAN_ENTRIES {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && prefix.is_empty() && name != "__MACOSX" {
                folders.push((entry.path(), format!("{name}/")));
            } else if kind.is_file() && is_license_name(&name) {
                found.push(format!("{prefix}{name}"));
            }
        }
    }
    sorted_hints(found)
}

/// Root files first, then by name; at most `MAX_HINTS`.
fn sorted_hints(mut found: Vec<String>) -> Vec<String> {
    found.sort_by_key(|path| (path.matches('/').count(), path.to_lowercase()));
    found.truncate(MAX_HINTS);
    found
}

fn is_license_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let licence = lower.contains("license") || lower.contains("licence");
    !lower.starts_with("._")
        && (lower.starts_with("readme")
            || lower.starts_with("license")
            || lower.starts_with("licence")
            || (licence
                && [".txt", ".md", ".pdf"]
                    .iter()
                    .any(|ext| lower.ends_with(ext))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_trims_empties_and_rejects_bad_links_dates_and_lengths() {
        let info = validated(&PackageInfo {
            store: Some("  itch.io ".into()),
            url: Some("https://kenney.nl".into()),
            license: Some("".into()),
            purchased: Some("2024-02-29".into()),
            notes: Some("Zeile 1\nZeile 2".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(info.store.as_deref(), Some("itch.io"));
        assert_eq!(info.license, None);
        assert_eq!(info.notes.as_deref(), Some("Zeile 1\nZeile 2"));
        let bad = |info: PackageInfo| validated(&info).is_err();
        assert!(bad(PackageInfo {
            url: Some("ftp://x".into()),
            ..Default::default()
        }));
        assert!(bad(PackageInfo {
            url: Some("javascript:alert(1)".into()),
            ..Default::default()
        }));
        assert!(bad(PackageInfo {
            url: Some(format!("https://{}", "a".repeat(2048))),
            ..Default::default()
        }));
        assert!(bad(PackageInfo {
            license: Some("x".repeat(201)),
            ..Default::default()
        }));
        assert!(bad(PackageInfo {
            author: Some("a\nb".into()),
            ..Default::default()
        }));
        for date in [
            "2023-02-29",
            "2024-13-01",
            "2024-1-01",
            "24-01-01x",
            "2024-04-31",
            "+024-01-01",
        ] {
            assert!(
                bad(PackageInfo {
                    purchased: Some(date.into()),
                    ..Default::default()
                }),
                "{date}"
            );
        }
        assert!(valid_date("2000-02-29") && !valid_date("1900-02-29"));
    }

    #[test]
    fn licence_and_readme_names_are_recognised() {
        for name in [
            "LICENSE",
            "License.txt",
            "Licence.md",
            "asset_license.txt",
            "README.md",
            "readme",
            "Royalty-Free License (Link).pdf",
        ] {
            assert!(is_license_name(name), "{name}");
        }
        for name in ["icon_license.png", "._License.txt", "notes.txt", "Hit.wav"] {
            assert!(!is_license_name(name), "{name}");
        }
    }
}
