//! Opt-in watching of external folders: a metadata inventory (no hashing) decides whether the
//! external-folder rescan runs. ZIP sources are not watched.
use super::{discover_external, file_identity, Library, RescanSummary, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

/// Watched sources and the state last reported for each, so identical changes are reported once.
pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS watched_sources (package_id INTEGER PRIMARY KEY REFERENCES packages(id) ON DELETE CASCADE, state TEXT);";

/// Minimum time between focus-triggered checks; activating a library always checks.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(60);

pub fn watch_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= WATCH_INTERVAL)
}

#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchNotice {
    pub package_id: i64,
    pub message: String,
    /// Set when the rescan ran; a replaced source or failed traversal has none.
    pub summary: Option<RescanSummary>,
}

const ACTIVE_FOLDER: &str = "FROM watched_sources w JOIN active_packages p ON p.id = w.package_id
    JOIN external_sources s ON s.package_id = p.id WHERE s.identity NOT LIKE 'zip:%'";

/// Order-independent fingerprint of a folder's discovered files.
fn inventory(root: &Path) -> Result<String> {
    let mut found = Vec::new();
    discover_external(root, Path::new(""), &mut found)?;
    found.sort_by(|a, b| a.relative.cmp(&b.relative));
    let mut hash = Sha256::new();
    for file in found {
        hash.update(format!(
            "{}\0{}\0{}\0{}\n",
            file.relative.display(),
            file.size,
            file.modified,
            file.identity
        ));
    }
    Ok(format!("inventory:{:x}", hash.finalize()))
}

impl Library {
    pub fn set_watched(&self, package_id: i64, watched: bool) -> Result<()> {
        let failed = |e: rusqlite::Error| format!("Beobachtung kann nicht gespeichert werden: {e}");
        if !watched {
            self.connection
                .execute(
                    "DELETE FROM watched_sources WHERE package_id = ?1",
                    [package_id],
                )
                .map_err(failed)?;
            return Ok(());
        }
        let folder: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM active_packages p JOIN external_sources s ON s.package_id = p.id
                WHERE p.id = ?1 AND s.identity NOT LIKE 'zip:%')",
                [package_id],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if !folder {
            return Err("Nur externe Ordner (keine ZIPs) können beobachtet werden".into());
        }
        self.connection
            .execute(
                "INSERT OR IGNORE INTO watched_sources(package_id) VALUES (?1)",
                [package_id],
            )
            .map_err(failed)?;
        Ok(())
    }

    pub fn watched_sources(&self) -> Result<Vec<i64>> {
        let failed = |e: rusqlite::Error| format!("Beobachtete Ordner nicht lesbar: {e}");
        self.connection
            .prepare(&format!("SELECT p.id {ACTIVE_FOLDER} ORDER BY p.id"))
            .map_err(failed)?
            .query_map([], |row| row.get(0))
            .map_err(failed)?
            .collect::<std::result::Result<_, _>>()
            .map_err(failed)
    }

    /// Inventories a watched folder and, when it differs from the last reported state, rescans
    /// it after `lock` (the caller's write gate) succeeded. Offline sources are skipped; a
    /// replaced root or failed traversal is reported once and never changes the catalog.
    pub fn check_watched<G>(
        &mut self,
        package_id: i64,
        lock: impl FnOnce() -> Result<G>,
    ) -> Result<Option<WatchNotice>> {
        let (name, root, identity, last): (String, String, String, Option<String>) = self
            .connection
            .query_row(
                &format!(
                    "SELECT p.name, s.root, s.identity, w.state {ACTIVE_FOLDER} AND p.id = ?1"
                ),
                [package_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|e| format!("Beobachteter Ordner nicht im Katalog: {e}"))?;
        let root = Path::new(&root);
        let Ok(metadata) = fs::symlink_metadata(root) else {
            return Ok(None);
        };
        let replaced = format!(
            "{name} wurde ersetzt und wird nicht übernommen. Bitte die Quelle neu verknüpfen."
        );
        let (state, problem) = if !metadata.is_dir() || metadata.file_type().is_symlink() {
            ("replaced".to_owned(), Some(replaced))
        } else {
            match file_identity(root, &metadata) {
                Ok(current) if current != identity => {
                    (format!("replaced:{current}"), Some(replaced))
                }
                Err(e) => (
                    format!("error:{e}"),
                    Some(format!("{name} kann nicht beobachtet werden: {e}")),
                ),
                Ok(_) => match inventory(root) {
                    Ok(state) => (state, None),
                    Err(e) => (
                        format!("error:{e}"),
                        Some(format!("{name} kann nicht vollständig gelesen werden: {e}")),
                    ),
                },
            }
        };
        if last.as_deref() == Some(state.as_str()) {
            return Ok(None);
        }
        let _gate = lock()?;
        let notice = match problem {
            Some(message) => Some(WatchNotice {
                package_id,
                message,
                summary: None,
            }),
            None => {
                let summary = self.rescan_package(package_id)?;
                let parts = [
                    (summary.added, "neu"),
                    (summary.changed, "geändert"),
                    (summary.missing, "fehlen"),
                ]
                .iter()
                .filter(|(count, _)| *count > 0)
                .map(|(count, label)| format!("{count} {label}"))
                .collect::<Vec<_>>();
                (!parts.is_empty()).then(|| WatchNotice {
                    package_id,
                    message: format!("Neue Dateien in {name}: {}.", parts.join(", ")),
                    summary: Some(summary),
                })
            }
        };
        self.connection
            .execute(
                "UPDATE watched_sources SET state = ?1 WHERE package_id = ?2",
                rusqlite::params![state, package_id],
            )
            .map_err(|e| format!("Beobachtung kann nicht gespeichert werden: {e}"))?;
        Ok(notice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Workspace(std::path::PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn setup() -> (Workspace, Library, i64, std::path::PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let workspace = Workspace(std::env::temp_dir().join(format!(
            "watch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let source = workspace.0.join("External");
        write(&source.join("Fx/Hit.wav"), b"hit");
        write(&source.join("Fx/Gone.wav"), b"gone");
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        let package = library.reference_folder(&source).unwrap().id;
        library.set_watched(package, true).unwrap();
        (workspace, library, package, source)
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn check(library: &mut Library, package: i64) -> Option<WatchNotice> {
        library.check_watched(package, || Ok(())).unwrap()
    }

    fn files(library: &Library) -> i64 {
        library
            .connection
            .query_row("SELECT COUNT(*) FROM asset_files", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn checks_are_throttled_after_the_first() {
        let now = Instant::now();
        assert!(watch_due(None, now));
        assert!(!watch_due(Some(now), now + Duration::from_secs(59)));
        assert!(watch_due(Some(now), now + WATCH_INTERVAL));
    }

    #[test]
    fn changes_are_rescanned_and_reported_once_with_new_changed_and_missing_apart() {
        let (_workspace, mut library, package, source) = setup();
        assert_eq!(check(&mut library, package), None, "matches the catalog");
        assert_eq!(check(&mut library, package), None);

        std::thread::sleep(Duration::from_millis(20));
        write(&source.join("Fx/Hit.wav"), b"HIT");
        write(&source.join("Fx/New.wav"), b"new");
        write(&source.join("Fx/Newer.wav"), b"newer");
        fs::remove_file(source.join("Fx/Gone.wav")).unwrap();
        let notice = check(&mut library, package).unwrap();
        let summary = notice.summary.unwrap();
        assert_eq!((summary.added, summary.changed, summary.missing), (2, 1, 1));
        assert_eq!(
            notice.message,
            "Neue Dateien in External: 2 neu, 1 geändert, 1 fehlen."
        );
        assert_eq!(files(&library), 4, "the rescan ran");
        assert_eq!(check(&mut library, package), None, "identical change");

        // Lingering missing files do not trigger a rescan; only a new difference does.
        write(&source.join("Fx/Third.wav"), b"third");
        let summary = check(&mut library, package).unwrap().summary.unwrap();
        assert_eq!((summary.added, summary.changed, summary.missing), (1, 0, 1));
    }

    #[test]
    fn replaced_unreadable_or_offline_sources_keep_the_catalog() {
        let (workspace, mut library, package, source) = setup();
        check(&mut library, package);
        fs::rename(&source, workspace.0.join("Moved")).unwrap();
        assert_eq!(check(&mut library, package), None, "offline is silent");

        write(&source.join("Fx/Hit.wav"), b"hit");
        write(&source.join("Fx/Other.wav"), b"other");
        let notice = check(&mut library, package).unwrap();
        assert!(notice.summary.is_none() && notice.message.contains("ersetzt"));
        assert_eq!(check(&mut library, package), None, "reported once");
        assert_eq!(files(&library), 2, "never adopted");

        fs::remove_dir_all(&source).unwrap();
        fs::rename(workspace.0.join("Moved"), &source).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            write(&source.join("Sub/B.wav"), b"b");
            fs::set_permissions(source.join("Sub"), fs::Permissions::from_mode(0o000)).unwrap();
            let notice = check(&mut library, package);
            fs::set_permissions(source.join("Sub"), fs::Permissions::from_mode(0o755)).unwrap();
            assert!(notice.unwrap().summary.is_none(), "incomplete traversal");
            assert_eq!(files(&library), 2);
        }
        assert!(library
            .check_watched(package, || Err::<(), _>("busy".to_owned()))
            .is_err());
        assert_eq!(files(&library), 2, "no gate, no change");
        let notice = check(&mut library, package);
        #[cfg(unix)]
        assert_eq!(notice.unwrap().summary.unwrap().added, 1);
        #[cfg(not(unix))]
        assert_eq!(notice, None);
    }

    #[test]
    fn only_active_external_folders_are_watched() {
        let (workspace, library, package, _) = setup();
        assert_eq!(library.watched_sources().unwrap(), vec![package]);
        let managed = workspace.0.join("Managed");
        write(&managed.join("A.wav"), b"a");
        let mut library = library;
        let managed = library.import_folder(&managed).unwrap().id;
        assert!(library.set_watched(managed, true).is_err());
        library.trash_package(package).unwrap();
        assert!(library.watched_sources().unwrap().is_empty());
        library.restore_trash(trash_id(&library)).unwrap();
        library.set_watched(package, false).unwrap();
        assert!(library.watched_sources().unwrap().is_empty());
    }

    #[test]
    fn a_version_4_catalog_opens_unwatched_and_becomes_version_5() {
        let (workspace, library, package, _) = setup();
        library
            .connection
            .execute_batch("DROP TABLE watched_sources; PRAGMA user_version = 4;")
            .unwrap();
        drop(library);
        let library = Library::open(&workspace.0.join("Library")).unwrap();
        let version: i64 = library
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 5);
        assert!(
            library.watched_sources().unwrap().is_empty(),
            "off by default"
        );
        assert_eq!(library.packages().unwrap()[0].id, package);
        library.set_watched(package, true).unwrap();
        drop(library);
        assert_eq!(
            Library::open(&workspace.0.join("Library"))
                .unwrap()
                .watched_sources()
                .unwrap(),
            vec![package],
            "persisted"
        );
    }

    fn trash_id(library: &Library) -> i64 {
        library
            .connection
            .query_row("SELECT id FROM trash", [], |row| row.get(0))
            .unwrap()
    }
}
