//! Small cached waveforms for sound cards, keyed by the source they were decoded from.
use super::{
    audio::{path_waveform, source_stamp},
    location_row,
    zip_sources::{archive_identity, is_zip, OpenArchives},
    AssetKind, AudioWaveform, Library, Result,
};
use rusqlite::{params, OptionalExtension};
use std::{
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Bump when `bars` or the decoder changes, so every cached row is computed again.
const ALGORITHM: i64 = 1;
const BARS: usize = 48;

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS audio_waveforms (file_id INTEGER PRIMARY KEY REFERENCES asset_files(id) ON DELETE CASCADE, stamp TEXT NOT NULL, algorithm INTEGER NOT NULL, peaks BLOB, error TEXT);";

/// Loudest sample per bar, 0–255.
fn bars(waveform: &AudioWaveform) -> Vec<u8> {
    let step = waveform.duration_seconds / BARS as f64;
    (0..BARS)
        .map(|bar| {
            let (from, to) = (step * bar as f64, step * (bar + 1) as f64);
            let level = waveform
                .peaks
                .iter()
                .filter(|peak| peak.start_seconds < to && peak.end_seconds > from)
                .fold(0f32, |level, peak| level.max(peak.max).max(-peak.min));
            (level.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect()
}

impl Library {
    /// Card peaks of a sound; a failure for an unchanged source is answered from the cache too.
    pub fn card_waveform(&self, file_id: i64, cancel: &AtomicBool) -> Result<Vec<u8>> {
        self.card_waveform_with(file_id, cancel, |path| path_waveform(path, cancel))
    }

    fn card_waveform_with(
        &self,
        file_id: i64,
        cancel: &AtomicBool,
        decode: impl FnOnce(&Path) -> Result<AudioWaveform>,
    ) -> Result<Vec<u8>> {
        let failed = |e: rusqlite::Error| format!("Wellenform nicht verfügbar: {e}");
        let (format, relative, external) = self
            .connection
            .query_row(
                "SELECT f.format, f.relative_path, s.root, s.identity, p.relative_path, e.identity, e.modified, f.size_bytes FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id LEFT JOIN external_sources s ON s.package_id = p.id LEFT JOIN external_files e ON e.file_id = f.id WHERE f.id = ?1",
                [file_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, location_row(row, 2)?)),
            )
            .map_err(|e| format!("Asset-Datei nicht im Katalog: {e}"))?;
        if AssetKind::for_format(&format) != Some(AssetKind::Sound) {
            return Err("Diese Asset-Datei ist kein Sound".into());
        }
        // A ZIP entry is stamped by its archive and CRC; extracting it just to stamp it would be wasted.
        let (zip, path) = match external {
            Some(zip) if is_zip(&zip.root_identity) => (Some(zip), PathBuf::new()),
            external => (None, self.locate(&relative, external)?),
        };
        let stamp = || -> Result<String> {
            Ok(match &zip {
                Some(zip) => format!(
                    "{}|{}",
                    archive_identity(Path::new(&zip.root))?,
                    zip.identity
                ),
                None => format!(
                    "{:?}",
                    source_stamp(&fs::symlink_metadata(&path).map_err(|e| e.to_string())?)?
                ),
            })
        };
        let before = stamp()?;
        let cached: Option<(Option<Vec<u8>>, Option<String>)> = self
            .connection
            .query_row(
                "SELECT peaks, error FROM audio_waveforms WHERE file_id = ?1 AND stamp = ?2 AND algorithm = ?3",
                params![file_id, before, ALGORITHM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(failed)?;
        if let Some((peaks, error)) = cached {
            return error.map_or(Ok(peaks.unwrap_or_default()), Err);
        }
        // A failing external parser is a cached failure, not a crashed worker.
        let decode = |path: &Path| {
            catch_unwind(AssertUnwindSafe(|| decode(path)))
                .unwrap_or_else(|_| Err("Wellenform-Analyse: Codec-Parserfehler".into()))
        };
        let decoded = match &zip {
            // The temporary copy is deleted by `with_zip_file`; an unavailable archive is not cached.
            Some(zip) => {
                self.with_zip_file(&mut OpenArchives::new(), &relative, zip, cancel, decode)?
            }
            None => decode(&path),
        };
        if cancel.load(Ordering::Relaxed) {
            return Err("Wellenform-Analyse abgebrochen".into());
        }
        if stamp()? != before {
            return Err("Sound wurde während der Wellenform-Analyse geändert".into());
        }
        let peaks = decoded.map(|waveform| bars(&waveform));
        // Stored under the stamp it was decoded from, so a later change can never be served this row.
        self.connection
            .execute(
                "INSERT OR REPLACE INTO audio_waveforms(file_id, stamp, algorithm, peaks, error)
                SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM asset_files WHERE id = ?1)",
                params![
                    file_id,
                    before,
                    ALGORITHM,
                    peaks.as_ref().ok(),
                    peaks.as_ref().err()
                ],
            )
            .map_err(failed)?;
        peaks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::CatalogQuery;
    use std::{io::Write, sync::atomic::AtomicU64};

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "lootbag-waveforms-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("Pack")).unwrap();
            Self(path)
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Mono 16-bit PCM at 8 kHz with every sample set to `level`.
    fn wav(samples: usize, level: i16) -> Vec<u8> {
        let data = samples as u32 * 2;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(
            b"WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data",
        );
        bytes.extend_from_slice(&data.to_le_bytes());
        for _ in 0..samples {
            bytes.extend_from_slice(&level.to_le_bytes());
        }
        bytes
    }

    fn only_file(library: &Library) -> i64 {
        library.query(CatalogQuery::default()).unwrap().items[0].files[0].id
    }

    fn row(library: &Library, file_id: i64) -> Option<(String, Option<Vec<u8>>, Option<String>)> {
        library
            .connection
            .query_row(
                "SELECT stamp, peaks, error FROM audio_waveforms WHERE file_id = ?1",
                [file_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .unwrap()
    }

    fn imported(bytes: &[u8]) -> (Workspace, Library, i64) {
        let workspace = Workspace::new();
        fs::write(workspace.0.join("Pack/tone.wav"), bytes).unwrap();
        let mut library = Library::create(&workspace.0.join("library")).unwrap();
        library.import_folder(&workspace.0.join("Pack")).unwrap();
        let id = only_file(&library);
        (workspace, library, id)
    }

    #[test]
    fn peaks_are_cached_per_stamp_and_recomputed_after_a_change() {
        let (_workspace, library, id) = imported(&wav(800, i16::MAX));
        let cancel = AtomicBool::new(false);
        let peaks = library.card_waveform(id, &cancel).unwrap();
        assert_eq!(peaks, vec![255; BARS]);
        let marker = vec![7u8; BARS];
        library
            .connection
            .execute("UPDATE audio_waveforms SET peaks = ?1", [&marker])
            .unwrap();
        assert_eq!(
            library.card_waveform(id, &cancel).unwrap(),
            marker,
            "served from the cache without decoding"
        );

        fs::write(library.resolve_preview(id).unwrap(), wav(1600, 0)).unwrap();
        assert_eq!(library.card_waveform(id, &cancel).unwrap(), vec![0; BARS]);
        library
            .connection
            .execute("UPDATE audio_waveforms SET algorithm = 0", [])
            .unwrap();
        library
            .connection
            .execute("UPDATE audio_waveforms SET peaks = ?1", [&marker])
            .unwrap();
        assert_eq!(
            library.card_waveform(id, &cancel).unwrap(),
            vec![0; BARS],
            "another algorithm version is computed again"
        );
    }

    #[test]
    fn a_version_3_catalog_gains_the_table_and_version_4() {
        let (workspace, library, id) = imported(&wav(800, 0));
        library
            .connection
            .execute_batch("DROP TABLE audio_waveforms; PRAGMA user_version = 3;")
            .unwrap();
        drop(library);
        let library = Library::open(&workspace.0.join("library")).unwrap();
        let version: i64 = library
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        assert!(library.card_waveform(id, &AtomicBool::new(false)).is_ok());
    }

    #[test]
    fn an_unchanged_failure_is_not_decoded_again() {
        let (_workspace, library, id) = imported(b"not audio at all");
        let cancel = AtomicBool::new(false);
        let error = library.card_waveform(id, &cancel).unwrap_err();
        assert_eq!(row(&library, id).unwrap().2, Some(error));
        let decoded = library.card_waveform_with(id, &cancel, |_| panic!("decoded again"));
        assert!(decoded.is_err());
    }

    #[test]
    fn stale_and_cancelled_results_are_discarded() {
        let (_workspace, library, id) = imported(&wav(800, 1000));
        let path = library.resolve_preview(id).unwrap();
        let changed = library.card_waveform_with(id, &AtomicBool::new(false), |path| {
            let waveform = path_waveform(path, &AtomicBool::new(false));
            fs::write(path, wav(1600, 1000)).unwrap();
            waveform
        });
        assert!(changed.unwrap_err().contains("geändert"));
        assert!(row(&library, id).is_none());

        let cancel = AtomicBool::new(false);
        let cancelled = library.card_waveform_with(id, &cancel, |path| {
            cancel.store(true, Ordering::Relaxed);
            path_waveform(path, &AtomicBool::new(false))
        });
        assert!(cancelled.unwrap_err().contains("abgebrochen"));
        assert!(row(&library, id).is_none());
        assert!(path.exists());
    }

    #[test]
    fn zip_entries_leave_no_extracted_audio_and_rows_cascade_on_permanent_deletion() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("pack.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("tone.wav", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&wav(800, i16::MIN)).unwrap();
        zip.finish().unwrap();
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&archive).unwrap();
        let id = only_file(&library);

        assert_eq!(
            library.card_waveform(id, &AtomicBool::new(false)).unwrap(),
            vec![255; BARS]
        );
        let cache = root.join(".cache");
        assert!(fs::read_dir(&cache).map_or(true, |mut entries| entries.next().is_none()));
        assert!(row(&library, id).unwrap().0.starts_with("zip:"));

        library.trash_file(id).unwrap();
        library.empty_trash().unwrap();
        let rows: i64 = library
            .connection
            .query_row("SELECT count(*) FROM audio_waveforms", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }
}
