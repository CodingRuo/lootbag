use super::{Library, Result};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageCategory {
    pub category: &'static str,
    pub bytes: u64,
    pub files: u64,
    pub paths: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalStorage {
    pub recorded_bytes: u64,
    pub recorded_versions: u64,
    pub references: u64,
    pub active_versions: u64,
    pub trash_versions: u64,
    pub shared_versions: u64,
    pub unqualified_references: u64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageReport {
    pub complete: bool,
    pub cancelled: bool,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub total_bytes: u64,
    pub categories: Vec<StorageCategory>,
    pub visited_entries: u64,
    pub excluded_entries: u64,
    pub issue_count: u64,
    pub issues: Vec<String>,
    pub external: Option<ExternalStorage>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageProgress {
    pub stage: &'static str,
    pub visited_entries: u64,
}

#[derive(Clone, Copy)]
struct Limits {
    rows: usize,
    entries: u64,
    directories: usize,
    depth: usize,
    retained_bytes: usize,
    work: Duration,
}
const LIMITS: Limits = Limits {
    rows: 100_000,
    entries: 200_000,
    directories: 50_000,
    depth: 48,
    retained_bytes: 32 * 1024 * 1024,
    work: Duration::from_secs(30),
};
const CATEGORIES: [&str; 6] = [
    "managed",
    "trash",
    "support",
    "staging",
    "unclassified",
    "shared",
];
const MAX_SAFE_BYTES: u64 = 9_007_199_254_740_991;
const MAX_DETAILS: usize = 30;

#[derive(Debug, PartialEq, Eq)]
struct Package {
    id: i64,
    path: PathBuf,
    external_root: Option<String>,
    external_identity: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
struct CatalogFile {
    id: i64,
    package_id: i64,
    path: PathBuf,
    size: u64,
    identity: Option<String>,
    modified: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    packages: Vec<Package>,
    files: Vec<CatalogFile>,
    trash: Vec<(i64, Option<i64>, Option<i64>, i64)>,
}

fn text(
    row: &rusqlite::Row<'_>,
    column: usize,
    bytes: &mut usize,
    cap: usize,
) -> Result<Option<String>> {
    let value = row.get_ref(column).map_err(|e| e.to_string())?;
    if value == rusqlite::types::ValueRef::Null {
        return Ok(None);
    }
    let value = value
        .as_str()
        .map_err(|_| "Ungültiger Text im Speicherkatalog")?;
    if value.len() > 4096 {
        return Err("Ein Katalogfeld überschreitet das Speicherbericht-Limit".into());
    }
    *bytes = bytes
        .checked_add(value.len())
        .filter(|bytes| *bytes <= cap)
        .ok_or("Katalogdaten überschreiten das Speicherbericht-Limit")?;
    Ok(Some(value.to_owned()))
}
pub(super) fn required_text(
    row: &rusqlite::Row<'_>,
    column: usize,
    bytes: &mut usize,
    cap: usize,
) -> Result<String> {
    text(row, column, bytes, cap)?.ok_or_else(|| "Katalogfeld fehlt".into())
}
pub(super) fn normal_path(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Ungültiger Pfad im Speicherkatalog".into());
    }
    Ok(path)
}

impl Library {
    fn storage_snapshot(
        &self,
        limits: Limits,
        cancelled: &AtomicBool,
        started: Instant,
    ) -> Result<Snapshot> {
        snapshot_guard(cancelled, started, limits.work)?;
        let _query_budget = QueryBudget::install(&self.connection, cancelled, started, limits.work);
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let mut bytes = 0;
        let mut count = 0;
        let mut packages = Vec::new();
        let mut statement = transaction.prepare("SELECT p.id,p.relative_path,s.root,s.identity FROM packages p LEFT JOIN external_sources s ON s.package_id=p.id ORDER BY p.id LIMIT ?1").map_err(|e| e.to_string())?;
        let mut rows = statement
            .query([(limits.rows + 1) as i64])
            .map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            snapshot_guard(cancelled, started, limits.work)?;
            count += 1;
            if count > limits.rows {
                return Err("Zu viele Katalogeinträge für einen Speicherbericht".into());
            }
            let path = normal_path(&required_text(row, 1, &mut bytes, limits.retained_bytes)?)?;
            let external_root = text(row, 2, &mut bytes, limits.retained_bytes)?;
            let external_identity = text(row, 3, &mut bytes, limits.retained_bytes)?;
            let parts = path.components().collect::<Vec<_>>();
            let expected = if external_root.is_some() {
                "external"
            } else {
                "packages"
            };
            if parts.len() != 2
                || parts[0].as_os_str() != expected
                || external_root.is_some() != external_identity.is_some()
            {
                return Err("Paketpfade sind für den Speicherbericht nicht eindeutig".into());
            }
            packages.push(Package {
                id: row.get(0).map_err(|e| e.to_string())?,
                path,
                external_root,
                external_identity,
            });
        }
        drop(rows);
        drop(statement);
        let mut files = Vec::new();
        let mut statement = transaction.prepare("SELECT f.id,a.package_id,f.relative_path,f.size_bytes,e.identity,e.modified FROM asset_files f LEFT JOIN assets a ON a.id=f.asset_id LEFT JOIN external_files e ON e.file_id=f.id ORDER BY f.id LIMIT ?1").map_err(|e| e.to_string())?;
        let mut rows = statement
            .query([(limits.rows + 1) as i64])
            .map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            snapshot_guard(cancelled, started, limits.work)?;
            count += 1;
            if count > limits.rows {
                return Err("Zu viele Katalogeinträge für einen Speicherbericht".into());
            }
            let size: i64 = row.get(3).map_err(|e| e.to_string())?;
            let package_id = row
                .get(1)
                .map_err(|_| "Datei ohne Paket im Speicherkatalog")?;
            files.push(CatalogFile {
                id: row.get(0).map_err(|e| e.to_string())?,
                package_id,
                path: normal_path(&required_text(row, 2, &mut bytes, limits.retained_bytes)?)?,
                size: u64::try_from(size).map_err(|_| "Ungültige Dateigröße im Katalog")?,
                identity: text(row, 4, &mut bytes, limits.retained_bytes)?,
                modified: text(row, 5, &mut bytes, limits.retained_bytes)?,
            });
        }
        drop(rows);
        drop(statement);
        let mut trash = Vec::new();
        let mut statement = transaction
            .prepare("SELECT id,package_id,file_id,deleted_at FROM trash ORDER BY id LIMIT ?1")
            .map_err(|e| e.to_string())?;
        let mut rows = statement
            .query([(limits.rows + 1) as i64])
            .map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            snapshot_guard(cancelled, started, limits.work)?;
            count += 1;
            if count > limits.rows {
                return Err("Zu viele Katalogeinträge für einen Speicherbericht".into());
            }
            trash.push((
                row.get(0).map_err(|e| e.to_string())?,
                row.get(1).map_err(|e| e.to_string())?,
                row.get(2).map_err(|e| e.to_string())?,
                row.get(3).map_err(|e| e.to_string())?,
            ));
        }
        drop(rows);
        drop(statement);
        for table in ["external_sources", "external_files"] {
            snapshot_guard(cancelled, started, limits.work)?;
            let rows: i64 = transaction
                .query_row(
                    &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT ?1)"),
                    [(limits.rows + 1) as i64],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            count = count
                .checked_add(usize::try_from(rows).map_err(|_| "Ungültige Kataloganzahl")?)
                .filter(|count| *count <= limits.rows)
                .ok_or("Zu viele Katalogeinträge für einen Speicherbericht")?;
        }
        let orphaned: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM external_sources s LEFT JOIN packages p ON p.id=s.package_id WHERE p.id IS NULL) OR EXISTS(SELECT 1 FROM external_files e LEFT JOIN asset_files f ON f.id=e.file_id WHERE f.id IS NULL)", [], |row| row.get(0)).map_err(|e| e.to_string())?;
        if orphaned {
            return Err("Externe Zuordnung enthält unbekannte Katalogeinträge".into());
        }
        snapshot_guard(cancelled, started, limits.work)?;
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(Snapshot {
            packages,
            files,
            trash,
        })
    }

    fn storage_report(
        &self,
        cancelled: &AtomicBool,
        progress: impl FnMut(StorageProgress),
    ) -> Result<StorageReport> {
        self.storage_report_with_limits(cancelled, progress, LIMITS)
    }
    fn storage_report_with_limits(
        &self,
        cancelled: &AtomicBool,
        mut progress: impl FnMut(StorageProgress),
        limits: Limits,
    ) -> Result<StorageReport> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (cancelled, progress, limits);
            Err("Speicherberichte sind derzeit nur für macOS qualifiziert".into())
        }
        #[cfg(target_os = "macos")]
        {
            let started = Instant::now();
            let mut report = StorageReport {
                complete: true,
                cancelled: false,
                started_at_ms: now_ms(),
                finished_at_ms: 0,
                total_bytes: 0,
                categories: CATEGORIES
                    .iter()
                    .map(|category| StorageCategory {
                        category,
                        bytes: 0,
                        files: 0,
                        paths: 0,
                    })
                    .collect(),
                visited_entries: 0,
                excluded_entries: 0,
                issue_count: 0,
                issues: Vec::new(),
                external: None,
            };
            if cancelled.load(Ordering::Relaxed) {
                report.cancelled = true;
                report.complete = false;
                report.finished_at_ms = now_ms();
                return Ok(report);
            }
            let root = anchored::root(&self.root)?;
            let root_stamp = Stamp::read(&root.metadata().map_err(|e| e.to_string())?);
            let snapshot = match self.storage_snapshot(limits, cancelled, started) {
                Ok(snapshot) => snapshot,
                Err(error)
                    if cancelled.load(Ordering::Relaxed) || started.elapsed() >= limits.work =>
                {
                    report.cancelled = cancelled.load(Ordering::Relaxed);
                    report.issue(error);
                    report.finished_at_ms = now_ms();
                    return Ok(report);
                }
                Err(error) => return Err(error),
            };
            let classification = Classification::new(&snapshot)?;
            report.external = Some(classification.external.clone());
            if classification.external.unqualified_references > 0 {
                report.issue("Externe Identitätsversionen sind teilweise ungeprüft; deren Katalogaufzeichnungen wurden einzeln gezählt".into());
            }
            progress(StorageProgress {
                stage: "walk",
                visited_entries: 0,
            });
            let mut scan = Scan {
                report: &mut report,
                cancelled,
                started,
                limits,
                retained_bytes: 0,
                root_device: root_stamp.device,
                groups: HashMap::new(),
                observed: Vec::new(),
                directories: Vec::new(),
                seen_paths: HashSet::new(),
                directory_ids: HashSet::new(),
                last_progress: Instant::now(),
                progress: &mut progress,
                classification: &classification,
                stopped: false,
            };
            scan.walk(&root, Path::new(""), 0)?;
            if !scan.stopped {
                (scan.progress)(StorageProgress {
                    stage: "verify",
                    visited_entries: scan.report.visited_entries,
                });
                scan.verify(&root)?;
            }
            for path in classification.known.keys() {
                if !scan.seen_paths.contains(path) {
                    scan.report.issue(format!(
                        "Katalogisierte Datei nicht gemessen: {}",
                        display_path(path)
                    ));
                }
            }
            for group in scan.groups.values() {
                if group.stamp.links > group.paths {
                    scan.report.issue("Datei hat weitere Hardlinks außerhalb der gemessenen Namen; keine Freigabe für Bereinigung".into());
                }
                let index = if group.categories.count_ones() == 1 {
                    group.categories.trailing_zeros() as usize
                } else {
                    5
                };
                let category = &mut scan.report.categories[index];
                category.bytes = add(category.bytes, group.stamp.size)?;
                category.files = add(category.files, 1)?;
                category.paths = add(category.paths, group.paths)?;
                scan.report.total_bytes = add(scan.report.total_bytes, group.stamp.size)?;
            }
            drop(scan);
            if !cancelled.load(Ordering::Relaxed) && started.elapsed() < limits.work {
                match self.storage_snapshot(limits, cancelled, started) {
                    Ok(current) if current == snapshot => {},
                    _ => report.issue("Katalogzuordnung wurde während der Messung geändert oder konnte nicht bestätigt werden; bitte aktualisieren".into()),
                }
            }
            if anchored::root(&self.root)
                .and_then(|file| file.metadata().map_err(|e| e.to_string()))
                .map(|metadata| Stamp::read(&metadata))
                .ok()
                != Some(root_stamp)
            {
                report.issue(
                    "Bibliotheksordner wurde während der Messung verändert oder ersetzt".into(),
                );
            }
            report.cancelled |= cancelled.load(Ordering::Relaxed);
            report.complete &= !report.cancelled;
            report.finished_at_ms = now_ms();
            progress(StorageProgress {
                stage: "finished",
                visited_entries: report.visited_entries,
            });
            Ok(report)
        }
    }
}

pub(super) struct QueryBudget<'a> {
    connection: &'a rusqlite::Connection,
    _state: Box<(&'a AtomicBool, Instant, Duration)>,
}
impl<'a> QueryBudget<'a> {
    pub(super) fn install(
        connection: &'a rusqlite::Connection,
        cancelled: &'a AtomicBool,
        started: Instant,
        work: Duration,
    ) -> Self {
        let state = Box::new((cancelled, started, work));
        // SAFETY: a stable Box outlives synchronous SQLite statements; Drop unregisters before freeing it.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                connection.handle(),
                1000,
                Some(query_budget),
                std::ptr::from_ref(state.as_ref()).cast_mut().cast(),
            );
        }
        Self {
            connection,
            _state: state,
        }
    }
}
unsafe extern "C" fn query_budget(pointer: *mut std::ffi::c_void) -> i32 {
    // SAFETY: installed pointer refers to the retained QueryBudget box on this worker; no callback can panic.
    let state = unsafe { &*pointer.cast::<(&AtomicBool, Instant, Duration)>() };
    i32::from(state.0.load(Ordering::Relaxed) || state.1.elapsed() >= state.2)
}
impl Drop for QueryBudget<'_> {
    fn drop(&mut self) {
        // SAFETY: the connection is still exclusively owned; clearing the callback releases the borrowed flag.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                self.connection.handle(),
                0,
                None,
                std::ptr::null_mut(),
            );
        }
    }
}

fn snapshot_guard(cancelled: &AtomicBool, started: Instant, work: Duration) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Speichermessung abgebrochen; Zuordnung noch nicht vollständig erfasst".into());
    }
    if started.elapsed() >= work {
        return Err(
            "Zeitlimit der Messung erreicht; Zuordnung noch nicht vollständig erfasst".into(),
        );
    }
    Ok(())
}
fn add(left: u64, right: u64) -> Result<u64> {
    left.checked_add(right)
        .filter(|sum| *sum <= MAX_SAFE_BYTES)
        .ok_or_else(|| {
            "Dateigrößen überschreiten die sichere Berichtszahl; kein vollständiges Ergebnis".into()
        })
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .take(300)
        .map(|value| if value.is_control() { '�' } else { value })
        .collect()
}
impl StorageReport {
    fn issue(&mut self, message: String) {
        self.complete = false;
        self.issue_count += 1;
        if self.issues.len() < MAX_DETAILS {
            self.issues.push(message);
        }
    }
}

struct Classification {
    packages: HashMap<PathBuf, bool>,
    known: HashMap<PathBuf, (bool, u64)>,
    external: ExternalStorage,
}
impl Classification {
    fn new(snapshot: &Snapshot) -> Result<Self> {
        let packages_by_id = snapshot
            .packages
            .iter()
            .map(|package| (package.id, package))
            .collect::<HashMap<_, _>>();
        let package_trash = snapshot
            .trash
            .iter()
            .filter_map(|(_, package, _, _)| *package)
            .collect::<HashSet<_>>();
        let file_trash = snapshot
            .trash
            .iter()
            .filter_map(|(_, _, file, _)| *file)
            .collect::<HashSet<_>>();
        if snapshot.trash.iter().any(|(_, package, file, _)| {
            package.is_some() == file.is_some()
                || package.is_some_and(|id| !packages_by_id.contains_key(&id))
                || file.is_some_and(|id| {
                    snapshot
                        .files
                        .binary_search_by_key(&id, |file| file.id)
                        .is_err()
                })
        }) {
            return Err("Papierkorbzuordnung ist für den Speicherbericht ungültig".into());
        }
        let mut packages = HashMap::new();
        let mut known = HashMap::new();
        let mut versions = BTreeMap::new();
        let mut external = ExternalStorage {
            recorded_bytes: 0,
            recorded_versions: 0,
            references: 0,
            active_versions: 0,
            trash_versions: 0,
            shared_versions: 0,
            unqualified_references: 0,
        };
        for package in &snapshot.packages {
            if package.external_root.is_none()
                && packages
                    .insert(package.path.clone(), package_trash.contains(&package.id))
                    .is_some()
            {
                return Err("Doppelte Paketwurzel im Speicherkatalog".into());
            }
        }
        for file in &snapshot.files {
            let package = packages_by_id
                .get(&file.package_id)
                .ok_or("Datei ohne gültiges Paket im Katalog")?;
            let relative = file
                .path
                .strip_prefix(&package.path)
                .map_err(|_| "Dateipfad gehört nicht zu seinem Paket")?;
            if relative.as_os_str().is_empty() {
                return Err("Dateipfad ist kein Paketinhalt".into());
            }
            let trashed = package_trash.contains(&package.id) || file_trash.contains(&file.id);
            if package.external_root.is_none() {
                if file.identity.is_some()
                    || file.modified.is_some()
                    || known
                        .insert(file.path.clone(), (trashed, file.size))
                        .is_some()
                {
                    return Err("Mehrdeutige Dateizuordnung im Katalog".into());
                }
            } else {
                external.references += 1;
                let identity = file
                    .identity
                    .as_deref()
                    .ok_or("Externe Dateiversion fehlt")?;
                let modified = file
                    .modified
                    .as_deref()
                    .ok_or("Externe Änderungsversion fehlt")?;
                let qualified = qualified_identity(identity) && qualified_modified(modified);
                if !qualified {
                    external.unqualified_references += 1;
                }
                let key = (
                    if qualified {
                        identity.to_owned()
                    } else {
                        format!("unqualified-file:{}", file.id)
                    },
                    modified.to_owned(),
                    file.size,
                );
                let entry = versions.entry(key).or_insert(0_u8);
                *entry |= if trashed { 2 } else { 1 };
            }
        }
        for ((_, _, size), categories) in versions {
            external.recorded_bytes = add(external.recorded_bytes, size)?;
            external.recorded_versions += 1;
            match categories {
                1 => external.active_versions += 1,
                2 => external.trash_versions += 1,
                _ => external.shared_versions += 1,
            }
        }
        Ok(Self {
            packages,
            known,
            external,
        })
    }
    fn category(&self, path: &Path) -> usize {
        let mut parts = path.components();
        let first = parts.next();
        let second = parts.next();
        if first.is_some_and(|value| value.as_os_str() == "packages") {
            if let Some(second) = second {
                let package_path = Path::new("packages").join(second.as_os_str());
                if let Some(package_trash) = self.packages.get(&package_path) {
                    return if *package_trash
                        || self.known.get(path).is_some_and(|(trash, _)| *trash)
                    {
                        1
                    } else {
                        0
                    };
                }
                if parts.next().is_some()
                    && second
                        .as_os_str()
                        .as_encoded_bytes()
                        .starts_with(b".import-")
                    && second.as_os_str().as_encoded_bytes().len() > 8
                {
                    return 3;
                }
            }
        }
        if second.is_none()
            && path.to_str().is_some_and(|name| {
                matches!(
                    name,
                    "catalog.sqlite3"
                        | "catalog.sqlite3-journal"
                        | "catalog.sqlite3-wal"
                        | "catalog.sqlite3-shm"
                ) || (name.starts_with("catalog-before-macos-metadata-")
                    && name.ends_with(".sqlite3")
                    && name.len() > 39)
            })
        {
            return 2;
        }
        4
    }
}
fn qualified_identity(value: &str) -> bool {
    let Some(value) = value.strip_prefix("mac-volume:") else {
        return false;
    };
    let Some((uuid, inode)) = value.split_once(':') else {
        return false;
    };
    uuid.len() == 32
        && uuid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !inode.is_empty()
        && inode.bytes().all(|byte| byte.is_ascii_digit())
        && inode.parse::<u64>().is_ok()
}

fn qualified_modified(value: &str) -> bool {
    let Some(value) = value
        .strip_prefix("SystemTime { tv_sec: ")
        .and_then(|value| value.strip_suffix(" }"))
    else {
        return false;
    };
    let Some((seconds, nanos)) = value.split_once(", tv_nsec: ") else {
        return false;
    };
    let (Ok(seconds_value), Ok(nanos_value)) = (seconds.parse::<i64>(), nanos.parse::<u32>())
    else {
        return false;
    };
    nanos_value < 1_000_000_000
        && seconds_value.to_string() == seconds
        && nanos_value.to_string() == nanos
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    device: u64,
    inode: u64,
    pub(in crate::library) size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    links: u64,
    mode: u32,
}
#[cfg(target_os = "macos")]
impl Stamp {
    pub(super) fn read(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            links: metadata.nlink(),
            mode: metadata.mode(),
        }
    }
    pub(super) fn is_dir(&self) -> bool {
        self.mode & 0xf000 == 0x4000
    }
    pub(super) fn is_file(&self) -> bool {
        self.mode & 0xf000 == 0x8000
    }
    pub(super) fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }
    pub(super) fn links(&self) -> u64 {
        self.links
    }
}
#[cfg(target_os = "macos")]
struct Group {
    stamp: Stamp,
    categories: u8,
    paths: u64,
}
#[cfg(target_os = "macos")]
struct Scan<'a> {
    report: &'a mut StorageReport,
    cancelled: &'a AtomicBool,
    started: Instant,
    limits: Limits,
    retained_bytes: usize,
    root_device: u64,
    groups: HashMap<(u64, u64), Group>,
    observed: Vec<(PathBuf, Stamp)>,
    directories: Vec<(PathBuf, Stamp)>,
    seen_paths: HashSet<PathBuf>,
    directory_ids: HashSet<(u64, u64)>,
    last_progress: Instant,
    progress: &'a mut dyn FnMut(StorageProgress),
    classification: &'a Classification,
    stopped: bool,
}
#[cfg(target_os = "macos")]
impl Scan<'_> {
    fn check(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        if self.cancelled.load(Ordering::Relaxed) {
            self.report.cancelled = true;
            self.stopped = true;
            self.report.complete = false;
        } else if self.started.elapsed() >= self.limits.work {
            self.report
                .issue("Zeitlimit der Messung erreicht; Ergebnis ist unvollständig".into());
            self.stopped = true;
        }
        !self.stopped
    }
    fn retain(&mut self, bytes: usize) -> Result<()> {
        self.retained_bytes = self
            .retained_bytes
            .checked_add(bytes)
            .filter(|size| *size <= self.limits.retained_bytes)
            .ok_or("Namens-/Beobachtungslimit des Speicherberichts erreicht")?;
        Ok(())
    }
    fn walk(&mut self, directory: &fs::File, relative: &Path, depth: usize) -> Result<()> {
        if !self.check() {
            return Ok(());
        }
        if depth > self.limits.depth || self.directories.len() >= self.limits.directories {
            self.report
                .issue("Ordnertiefe/-anzahl überschreitet das Messlimit".into());
            self.stopped = true;
            return Ok(());
        }
        let before = Stamp::read(&directory.metadata().map_err(|e| e.to_string())?);
        if before.device != self.root_device || !self.directory_ids.insert(before.identity()) {
            self.report.excluded_entries += 1;
            self.report
                .issue("Eingehängter oder wiederholt erreichbarer Ordner wurde ausgelassen".into());
            return Ok(());
        }
        self.retain(
            relative.as_os_str().as_encoded_bytes().len() + std::mem::size_of::<(PathBuf, Stamp)>(),
        )?;
        self.directories.push((relative.to_owned(), before));
        let listing = anchored::Listing::new(directory)?;
        let mut names = HashSet::new();
        while self.check() {
            let Some(name) = listing.next()? else {
                break;
            };
            if self.report.visited_entries >= self.limits.entries {
                self.report
                    .issue("Dateianzahl überschreitet das Messlimit".into());
                self.stopped = true;
                break;
            }
            self.report.visited_entries += 1;
            self.retain(name.as_encoded_bytes().len() + 64)?;
            if !names.insert(name.clone()) {
                self.report
                    .issue("Ordnerinhalt wurde während der Messung verändert".into());
                continue;
            }
            let path = relative.join(&name);
            if path.as_os_str().as_encoded_bytes().len() > 4096 {
                self.report.excluded_entries += 1;
                self.report
                    .issue("Dateipfad überschreitet das Messlimit".into());
                continue;
            }
            let file = match anchored::child(directory, &name) {
                Ok(file) => file,
                Err(_) => {
                    self.report.excluded_entries += 1;
                    self.report.issue(format!(
                        "Nicht sicher lesbarer Eintrag ausgelassen: {}",
                        display_path(&path)
                    ));
                    continue;
                }
            };
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            let stamp = Stamp::read(&metadata);
            if stamp.device != self.root_device {
                self.report.excluded_entries += 1;
                self.report.issue(format!(
                    "Anderes Dateisystem ausgelassen: {}",
                    display_path(&path)
                ));
            } else if metadata.is_dir() {
                self.walk(&file, &path, depth + 1)?;
            } else if metadata.is_file() {
                self.retain(
                    path.as_os_str().as_encoded_bytes().len() * 2
                        + std::mem::size_of::<(PathBuf, Stamp)>()
                        + 128,
                )?;
                self.seen_paths.insert(path.clone());
                if self
                    .classification
                    .known
                    .get(&path)
                    .is_some_and(|(_, size)| *size != stamp.size)
                {
                    self.report.issue(format!(
                        "Dateigröße weicht vom Katalog ab: {}",
                        display_path(&path)
                    ));
                }
                let category = self.classification.category(&path);
                let group = self.groups.entry(stamp.identity()).or_insert(Group {
                    stamp,
                    categories: 0,
                    paths: 0,
                });
                if group.stamp != stamp {
                    self.report.issue(format!(
                        "Datei wurde während der Messung geändert: {}",
                        display_path(&path)
                    ));
                }
                group.categories |= 1 << category;
                group.paths += 1;
                self.observed.push((path.clone(), stamp));
            } else {
                self.report.excluded_entries += 1;
                self.report
                    .issue(format!("Spezialdatei ausgelassen: {}", display_path(&path)));
            }
            if anchored::child(directory, &name)
                .and_then(|file| file.metadata().map_err(|e| e.to_string()))
                .map(|metadata| Stamp::read(&metadata))
                .ok()
                != Some(stamp)
            {
                self.report.issue(format!(
                    "Eintrag wurde während der Messung ersetzt: {}",
                    display_path(&path)
                ));
            }
            if self.last_progress.elapsed() >= Duration::from_millis(100) {
                (self.progress)(StorageProgress {
                    stage: "walk",
                    visited_entries: self.report.visited_entries,
                });
                self.last_progress = Instant::now();
            }
        }
        if Stamp::read(&directory.metadata().map_err(|e| e.to_string())?) != before {
            self.report.issue(format!(
                "Ordnerinhalt wurde während der Messung verändert: {}",
                display_path(relative)
            ));
        }
        Ok(())
    }
    fn verify(&mut self, root: &fs::File) -> Result<()> {
        for index in 0..self.observed.len() {
            if !self.check() {
                break;
            }
            let (path, expected) = &self.observed[index];
            if anchored::relative(root, path)
                .and_then(|file| file.metadata().map_err(|e| e.to_string()))
                .map(|metadata| Stamp::read(&metadata))
                .ok()
                != Some(*expected)
            {
                self.report.issue(format!(
                    "Datei wurde inzwischen verändert oder ersetzt: {}",
                    display_path(path)
                ));
            }
        }
        for index in 0..self.directories.len() {
            if !self.check() {
                break;
            }
            let (path, expected) = &self.directories[index];
            if anchored::relative(root, path)
                .and_then(|file| file.metadata().map_err(|e| e.to_string()))
                .map(|metadata| Stamp::read(&metadata))
                .ok()
                != Some(*expected)
            {
                self.report.issue(format!(
                    "Ordner wurde inzwischen verändert oder ersetzt: {}",
                    display_path(path)
                ));
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(super) mod anchored {
    use super::*;
    use std::{
        ffi::{CString, OsStr, OsString},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::{OsStrExt, OsStringExt},
        },
    };
    extern "C" {
        fn openat(fd: i32, name: *const std::ffi::c_char, flags: i32, ...) -> i32;
        #[cfg_attr(target_arch = "x86_64", link_name = "fstatat$INODE64")]
        fn fstatat(fd: i32, name: *const std::ffi::c_char, stat: *mut Stat, flags: i32) -> i32;
        #[cfg_attr(target_arch = "x86_64", link_name = "fdopendir$INODE64")]
        fn fdopendir(fd: i32) -> *mut std::ffi::c_void;
        #[cfg_attr(target_arch = "x86_64", link_name = "readdir$INODE64")]
        fn readdir(directory: *mut std::ffi::c_void) -> *const Entry;
        fn closedir(directory: *mut std::ffi::c_void) -> i32;
        fn __error() -> *mut i32;
    }
    #[repr(C)]
    struct Entry {
        inode: u64,
        offset: u64,
        record_length: u16,
        name_length: u16,
        kind: u8,
        name: [u8; 1024],
    }
    // Darwin SDK sys/stat.h 64-bit inode layout, for the two supported Mac architectures.
    #[repr(C)]
    struct Stat {
        device: i32,
        mode: u16,
        links: u16,
        inode: u64,
        uid: u32,
        gid: u32,
        rdev: i32,
        accessed: [i64; 2],
        modified: [i64; 2],
        changed: [i64; 2],
        born: [i64; 2],
        size: i64,
        blocks: i64,
        block_size: i32,
        flags: u32,
        generation: u32,
        spare: i32,
        quad_spare: [i64; 2],
    }
    fn name(name: &OsStr) -> Result<CString> {
        if name.as_bytes().len() > 1023
            || name.as_bytes().is_empty()
            || name.as_bytes().contains(&b'/')
        {
            return Err("Ungültiger Dateiname".into());
        }
        CString::new(name.as_bytes()).map_err(|_| "Ungültiger Dateiname".into())
    }
    // Metadata-only lookups must not open/close an inode that SQLite may have locked:
    // POSIX record locks are process-owned and any close of that inode releases them.
    pub(in crate::library) fn optional_child_stamp(
        parent: &fs::File,
        leaf: &OsStr,
    ) -> Result<Option<Stamp>> {
        let leaf = name(leaf)?;
        let mut stat = std::mem::MaybeUninit::<Stat>::uninit();
        // SAFETY: qualified SDK layout, live held parent and bounded single leaf;
        // AT_SYMLINK_NOFOLLOW avoids following a replacement link and acquires no descriptor.
        if unsafe { fstatat(parent.as_raw_fd(), leaf.as_ptr(), stat.as_mut_ptr(), 0x20) } != 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error.to_string())
            };
        }
        let stat = unsafe { stat.assume_init() };
        if stat.mode & 0xf000 == 0xa000 {
            return Err("Symbolischer Link ist nicht zulässig".into());
        }
        Ok(Some(Stamp {
            device: stat.device as u64,
            inode: stat.inode,
            size: stat.size as u64,
            modified: (stat.modified[0], stat.modified[1]),
            changed: (stat.changed[0], stat.changed[1]),
            links: stat.links as u64,
            mode: stat.mode as u32,
        }))
    }
    pub(in crate::library) fn child_stamp(parent: &fs::File, leaf: &OsStr) -> Result<Stamp> {
        optional_child_stamp(parent, leaf)?.ok_or_else(|| "Datei wurde entfernt".into())
    }
    const FLAGS: i32 = 4 | 0x100 | 0x1000000; // Darwin O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC.
    pub(in crate::library) fn child(parent: &fs::File, leaf: &OsStr) -> Result<fs::File> {
        open_child(parent, leaf, FLAGS)
    }
    pub(in crate::library) fn directory(parent: &fs::File, leaf: &OsStr) -> Result<fs::File> {
        open_child(parent, leaf, FLAGS | 0x100000) // O_DIRECTORY: a replaced regular inode is never opened.
    }
    fn open_child(parent: &fs::File, leaf: &OsStr, flags: i32) -> Result<fs::File> {
        let name = name(leaf)?;
        // SAFETY: live owned parent descriptor, bounded single NUL-terminated leaf; no symlinks or blocking FIFO open.
        let fd = unsafe { openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: successful openat transfers one descriptor.
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
    pub(super) fn journal(parent: &fs::File) -> Result<Option<fs::File>> {
        // SAFETY: fixed leaf beneath a held directory; no links or blocking special-file open.
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                c"catalog.sqlite3-journal".as_ptr(),
                FLAGS,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error.to_string())
            };
        }
        // SAFETY: successful openat transfers one descriptor.
        Ok(Some(unsafe { fs::File::from_raw_fd(fd) }))
    }
    pub(in crate::library) fn root_file(path: &Path) -> Result<fs::File> {
        let parent = root(
            path.parent()
                .ok_or("Quellpfad hat keinen übergeordneten Ordner")?,
        )?;
        child(
            &parent,
            path.file_name().ok_or("Quellpfad hat keinen Namen")?,
        )
    }
    pub(in crate::library) fn root(path: &Path) -> Result<fs::File> {
        if path.as_os_str().as_encoded_bytes().len() > 4096 {
            return Err("Bibliothekspfad überschreitet das Messlimit".into());
        }
        if !path.is_absolute() {
            return Err("Bibliothekswurzel ist nicht absolut".into());
        }
        let mut file = fs::File::open("/").map_err(|e| e.to_string())?;
        for part in path.components() {
            if let Component::Normal(name) = part {
                file = directory(&file, name)?;
                if !file.metadata().map_err(|e| e.to_string())?.is_dir() {
                    return Err("Bibliotheksordner wurde ersetzt".into());
                }
            } else if !matches!(part, Component::RootDir) {
                return Err("Ungültige Bibliothekswurzel".into());
            }
        }
        Ok(file)
    }
    pub(in crate::library) fn relative(root: &fs::File, path: &Path) -> Result<fs::File> {
        let mut file = root.try_clone().map_err(|e| e.to_string())?;
        for part in path.components() {
            let Component::Normal(name) = part else {
                return Err("Ungültiger relativer Pfad".into());
            };
            if !file.metadata().map_err(|e| e.to_string())?.is_dir() {
                return Err("Übergeordneter Ordner wurde ersetzt".into());
            }
            file = child(&file, name)?;
        }
        Ok(file)
    }
    pub(super) fn decode_name(bytes: &[u8]) -> Result<OsString> {
        if bytes.is_empty() || bytes.len() > 1023 || bytes.contains(&0) || bytes.contains(&b'/') {
            return Err("Ungültiger Ordnername".into());
        }
        Ok(OsString::from_vec(bytes.to_vec()))
    }
    pub(in crate::library) struct Listing(*mut std::ffi::c_void);
    impl Listing {
        pub(in crate::library) fn new(parent: &fs::File) -> Result<Self> {
            // SAFETY: O_DIRECTORY with a fresh open creates an independent listing position.
            let fd = unsafe { openat(parent.as_raw_fd(), c".".as_ptr(), FLAGS | 0x100000) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let file = unsafe { fs::File::from_raw_fd(fd) };
            let directory = unsafe { fdopendir(fd) };
            if directory.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            std::mem::forget(file);
            Ok(Self(directory))
        }
        pub(in crate::library) fn next(&self) -> Result<Option<OsString>> {
            loop {
                // SAFETY: thread-local errno and owned live DIR; copy bounded bytes before the next readdir call.
                unsafe {
                    *__error() = 0;
                }
                let entry = unsafe { readdir(self.0) };
                if entry.is_null() {
                    return if unsafe { *__error() } == 0 {
                        Ok(None)
                    } else {
                        Err(std::io::Error::last_os_error().to_string())
                    };
                }
                let length = unsafe { (*entry).name_length as usize };
                if length == 0
                    || length > 1023
                    || unsafe { (*entry).record_length as usize }
                        < std::mem::offset_of!(Entry, name) + length + 1
                {
                    return Err("Ungültiger Ordnername".into());
                }
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        std::ptr::addr_of!((*entry).name).cast::<u8>(),
                        length,
                    )
                };
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                return Ok(Some(decode_name(bytes)?));
            }
        }
    }
    impl Drop for Listing {
        fn drop(&mut self) {
            unsafe {
                closedir(self.0);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn rollback_journal_stamp(journal: &fs::File) -> Result<Stamp> {
    use std::os::unix::fs::FileExt;
    let metadata = journal.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Rollback-Journal ist keine reguläre Datei".into());
    }
    let stamp = Stamp::read(&metadata);
    if metadata.len() > 512 {
        let mut header = [0_u8; 8];
        journal
            .read_exact_at(&mut header, 0)
            .map_err(|e| e.to_string())?;
        if header.iter().any(|byte| *byte != 0) {
            return Err("Rollback-Journal benötigt möglicherweise Wiederherstellung; Speichermessung ist ohne Änderungen nicht verfügbar".into());
        }
    }
    if Stamp::read(&journal.metadata().map_err(|e| e.to_string())?) != stamp {
        return Err("Rollback-Journal wurde beim Prüfen verändert".into());
    }
    Ok(stamp)
}

#[cfg(target_os = "macos")]
fn immutable_catalog(root: &Path, catalog: &fs::File) -> Result<Library> {
    use std::os::fd::AsRawFd;
    // A held descriptor fixes the inode; immutable URI mode suppresses SQLite locking and sidecar writes.
    let uri = format!("file:/dev/fd/{}?immutable=1", catalog.as_raw_fd());
    let connection = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("Katalog kann nicht unverändernd geöffnet werden: {e}"))?;
    // SAFETY: this private connection is exclusively owned and has no prepared statements yet.
    unsafe {
        rusqlite::ffi::sqlite3_limit(
            connection.handle(),
            rusqlite::ffi::SQLITE_LIMIT_LENGTH,
            32_768,
        );
        rusqlite::ffi::sqlite3_limit(
            connection.handle(),
            rusqlite::ffi::SQLITE_LIMIT_SQL_LENGTH,
            32_768,
        );
    }
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|e| e.to_string())?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version != 1 {
        return Err("Katalogversion ist für die Speichermessung nicht verfügbar".into());
    }
    connection.prepare("SELECT a.id FROM active_assets a LEFT JOIN folders f ON 0 LEFT JOIN folder_assets fa ON 0 LIMIT 0").map_err(|_| "Bitte die Bibliothek zuerst in der App aktualisieren")?;
    Ok(Library {
        root: root.to_owned(),
        connection,
    })
}

// Private readers keep this lease until their SQLite and raw descriptors close.
// Promotion is atomic and fails busy rather than copying while a cancelled reader
// can still close the same inode and release POSIX SQLite record locks.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct CatalogUse {
    readers: usize,
    publishing: bool,
}
#[cfg(target_os = "macos")]
static CATALOG_USERS: std::sync::Mutex<std::collections::BTreeMap<(u64, u64), CatalogUse>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
#[cfg(target_os = "macos")]
pub(super) struct CatalogLease {
    identity: (u64, u64),
    publishing: bool,
}
#[cfg(target_os = "macos")]
impl CatalogLease {
    fn read(identity: (u64, u64)) -> Result<Self> {
        let mut users = CATALOG_USERS
            .lock()
            .map_err(|_| "Katalogprüfung ist beschäftigt")?;
        let entry = users.entry(identity).or_default();
        if entry.publishing {
            return Err("Katalogprüfung ist beschäftigt; bitte erneut vorbereiten".into());
        }
        entry.readers += 1;
        Ok(Self {
            identity,
            publishing: false,
        })
    }
    fn reserve(&mut self) -> Result<()> {
        let mut users = CATALOG_USERS
            .lock()
            .map_err(|_| "Katalogprüfung ist beschäftigt")?;
        let entry = users
            .get_mut(&self.identity)
            .ok_or("Katalogprüfung ist nicht mehr registriert")?;
        if entry.readers != 1 || entry.publishing {
            return Err(
                "Eine Katalogprüfung läuft noch; bitte nach ihrem Ende erneut vorbereiten".into(),
            );
        }
        entry.publishing = true;
        self.publishing = true;
        Ok(())
    }
}
#[cfg(target_os = "macos")]
pub(super) fn reserve_catalog_publication(directory: &fs::File) -> Result<CatalogLease> {
    let stamp = anchored::child_stamp(directory, std::ffi::OsStr::new("catalog.sqlite3"))?;
    if !stamp.is_file() {
        return Err("Katalog ist keine reguläre Datei".into());
    }
    let identity = stamp.identity();
    let mut users = CATALOG_USERS
        .lock()
        .map_err(|_| "Katalogprüfung ist beschäftigt")?;
    let entry = users.entry(identity).or_default();
    if entry.readers != 0 || entry.publishing {
        return Err(
            "Eine Katalogprüfung läuft noch; bitte nach ihrem Ende erneut vorbereiten".into(),
        );
    }
    entry.readers = 1;
    entry.publishing = true;
    Ok(CatalogLease {
        identity,
        publishing: true,
    })
}
#[cfg(target_os = "macos")]
impl Drop for CatalogLease {
    fn drop(&mut self) {
        if let Ok(mut users) = CATALOG_USERS.lock() {
            if let Some(entry) = users.get_mut(&self.identity) {
                entry.readers -= 1;
                if self.publishing {
                    entry.publishing = false;
                }
                if entry.readers == 0 {
                    users.remove(&self.identity);
                }
            }
        }
    }
}
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub(super) enum ReservedOpenError {
    Busy(String),
    Unavailable(String),
}
// Both private read-only workflows retain the same descriptor/journal qualification.
#[cfg(target_os = "macos")]
pub(super) struct CatalogReader {
    pub(super) library: Library,
    pub(super) directory: fs::File,
    catalog: fs::File,
    root_stamp: Stamp,
    catalog_stamp: Stamp,
    journal_stamp: Option<Stamp>,
    journal: Option<fs::File>,
    // Field destruction order is significant: release only after every catalogue descriptor.
    lease: CatalogLease,
}
#[cfg(target_os = "macos")]
impl CatalogReader {
    pub(super) fn open(root: &Path) -> Result<Self> {
        let directory = anchored::root(root)?;
        let expected = anchored::child_stamp(&directory, std::ffi::OsStr::new("catalog.sqlite3"))?;
        let lease = CatalogLease::read(expected.identity())?;
        Self::open_owned(root, directory, expected, lease)
    }
    // Recovery reserves before raw catalogue descriptors, so cancelled readers cannot
    // close the same inode during its normal SQLite transaction.
    pub(super) fn open_reserved(root: &Path) -> std::result::Result<Self, ReservedOpenError> {
        let directory = anchored::root(root).map_err(ReservedOpenError::Unavailable)?;
        let expected = anchored::child_stamp(&directory, std::ffi::OsStr::new("catalog.sqlite3"))
            .map_err(ReservedOpenError::Unavailable)?;
        let lease = reserve_catalog_publication(&directory).map_err(ReservedOpenError::Busy)?;
        Self::open_owned(root, directory, expected, lease).map_err(ReservedOpenError::Unavailable)
    }
    fn open_owned(
        root: &Path,
        directory: fs::File,
        expected_catalog: Stamp,
        lease: CatalogLease,
    ) -> Result<Self> {
        use std::io::Read;
        let root_stamp = Stamp::read(&directory.metadata().map_err(|e| e.to_string())?);
        let mut catalog = anchored::child(&directory, std::ffi::OsStr::new("catalog.sqlite3"))?;
        if !catalog.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Katalog ist keine reguläre Datei".into());
        }
        let catalog_stamp = Stamp::read(&catalog.metadata().map_err(|e| e.to_string())?);
        if catalog_stamp != expected_catalog {
            return Err("Katalog wurde beim Öffnen ersetzt".into());
        }
        let mut header = [0_u8; 100];
        catalog
            .read_exact(&mut header)
            .map_err(|_| "Katalogkopf ist unvollständig")?;
        if &header[..16] != b"SQLite format 3\0" || header[18] != 1 || header[19] != 1 {
            return Err("Nur SQLite-Bibliotheken mit Rollback-Journal sind für unverändernde Prüfungen qualifiziert".into());
        }
        let journal = anchored::journal(&directory)?;
        let journal_stamp = journal.as_ref().map(rollback_journal_stamp).transpose()?;
        let library = immutable_catalog(root, &catalog)?;
        let reader = Self {
            library,
            directory,
            catalog,
            root_stamp,
            catalog_stamp,
            journal_stamp,
            journal,
            lease,
        };
        reader.validate()?;
        Ok(reader)
    }
    pub(super) fn reserve_publication(&mut self) -> Result<()> {
        self.lease.reserve()
    }
    pub(super) fn catalog_file(&self) -> &fs::File {
        &self.catalog
    }
    pub(super) fn journal_file(&self) -> Option<&fs::File> {
        self.journal.as_ref()
    }
    pub(super) fn validate(&self) -> Result<()> {
        self.validate_with_root(self.root_stamp)
    }
    // Only the guarded importer supplies a separately proven owned-stage root transition.
    pub(super) fn validate_with_root(&self, expected_root: Stamp) -> Result<()> {
        if self
            .journal
            .as_ref()
            .map(rollback_journal_stamp)
            .transpose()
            .ok()
            != Some(self.journal_stamp)
            || anchored::optional_child_stamp(
                &self.directory,
                std::ffi::OsStr::new("catalog.sqlite3-journal"),
            )
            .ok()
                != Some(self.journal_stamp)
        {
            return Err("Rollback-Journal wurde verändert oder benötigt Wiederherstellung; bitte die Bibliothek zuerst öffnen".into());
        }
        if anchored::root(&self.library.root)
            .and_then(|file| file.metadata().map_err(|e| e.to_string()))
            .map(|m| Stamp::read(&m))
            .ok()
            != Some(expected_root)
        {
            return Err("Bibliotheksordner wurde verändert oder ersetzt".into());
        }
        if Stamp::read(&self.catalog.metadata().map_err(|e| e.to_string())?) != self.catalog_stamp
            || anchored::child_stamp(&self.directory, std::ffi::OsStr::new("catalog.sqlite3")).ok()
                != Some(self.catalog_stamp)
        {
            return Err("Katalogdatei wurde verändert oder ersetzt; bitte aktualisieren".into());
        }
        Ok(())
    }
}
pub(crate) fn read_storage_report(
    root: &Path,
    cancelled: &AtomicBool,
    progress: impl FnMut(StorageProgress),
) -> Result<StorageReport> {
    let started_at_ms = now_ms();
    #[cfg(target_os = "macos")]
    {
        let reader = CatalogReader::open(root)?;
        let mut report = reader.library.storage_report(cancelled, progress)?;
        if let Err(error) = reader.validate() {
            report.issue(error);
        }
        report.started_at_ms = started_at_ms;
        report.finished_at_ms = now_ms();
        Ok(report)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, cancelled, progress, started_at_ms);
        Err("Speicherberichte sind derzeit nur für macOS qualifiziert".into())
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "storage_tests.rs"]
mod tests;
