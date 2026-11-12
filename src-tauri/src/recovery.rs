#[cfg(target_os = "macos")]
use super::storage::{anchored, CatalogReader, QueryBudget, Stamp};
use super::{ImportControl, Result};
#[cfg(target_os = "macos")]
use std::{ffi::OsStr, fs};
use std::{
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
struct ReadReservation {
    // Close the normal SQLite owner before any same-inode raw descriptor or its lease.
    connection: Option<rusqlite::Connection>,
    retained_aliases: Vec<fs::File>,
    reader: CatalogReader,
    protected: Vec<Stamp>,
}
#[cfg(target_os = "macos")]
impl ReadReservation {
    fn open(
        root: &Path,
        control: &ImportControl,
        started: Instant,
        work: Duration,
    ) -> Checked<Self> {
        check(control, started, work)?;
        let directory = anchored::root(root).map_err(fault("unsupported"))?;
        let mut protected = Vec::new();
        for name in [
            "catalog.sqlite3",
            "catalog.sqlite3-journal",
            "catalog.sqlite3-wal",
            "catalog.sqlite3-shm",
        ] {
            if let Some(stamp) = anchored::optional_child_stamp(&directory, OsStr::new(name))
                .map_err(fault("unsupported"))?
            {
                if !stamp.is_file() || stamp.links() != 1 {
                    return Err(RecoveryFault::new(
                        "unsupported",
                        "Katalog/SQLite-Datei ist verknüpft oder keine reguläre Datei",
                    ));
                }
                if name.ends_with("-wal") || name.ends_with("-shm") {
                    return Err(RecoveryFault::new(
                        "unsupported",
                        "WAL/SHM-Bibliotheken sind für diese Sicherung nicht verfügbar",
                    ));
                }
                protected.push(stamp);
            } else if name == "catalog.sqlite3" {
                return Err(RecoveryFault::new(
                    "unsupported",
                    "Bibliothekskatalog fehlt",
                ));
            }
        }
        let reader = CatalogReader::open_reserved(root).map_err(|e| match e {
            super::storage::ReservedOpenError::Busy(e) => RecoveryFault::new("busy", e),
            super::storage::ReservedOpenError::Unavailable(e) => {
                RecoveryFault::new("unsupported", e)
            }
        })?;
        // Normal named locking plus read-only SHM; never immutable/devfd/nolock.
        let connection =
            normal_read_only(&root.join("catalog.sqlite3")).map_err(fault("sourceChanged"))?;
        connection
            .busy_timeout(Duration::from_millis(100))
            .map_err(|e| sql_fault(e, control, started, work))?;
        unsafe {
            rusqlite::ffi::sqlite3_limit(
                connection.handle(),
                rusqlite::ffi::SQLITE_LIMIT_LENGTH,
                32768,
            );
            rusqlite::ffi::sqlite3_limit(
                connection.handle(),
                rusqlite::ffi::SQLITE_LIMIT_SQL_LENGTH,
                32768,
            );
        }
        {
            let _budget =
                QueryBudget::install(&connection, control.cancellation_flag(), started, work);
            connection
                .pragma_update(None, "query_only", true)
                .map_err(|e| sql_fault(e, control, started, work))?;
            connection
                .execute_batch("BEGIN DEFERRED")
                .map_err(|e| sql_fault(e, control, started, work))?;
            let _: String = connection
                .query_row(
                    "SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| sql_fault(e, control, started, work))?;
        }
        let source = Self {
            connection: Some(connection),
            retained_aliases: Vec::new(),
            reader,
            protected,
        };
        source.validate().map_err(fault("sourceChanged"))?;
        check(control, started, work)?;
        Ok(source)
    }
    fn validate(&self) -> Result<()> {
        self.reader.validate()?;
        for name in [
            "catalog.sqlite3",
            "catalog.sqlite3-journal",
            "catalog.sqlite3-wal",
            "catalog.sqlite3-shm",
        ] {
            if let Some(stamp) =
                anchored::optional_child_stamp(&self.reader.directory, OsStr::new(name))?
            {
                if !stamp.is_file() || stamp.links() != 1 || !self.protected.contains(&stamp) {
                    return Err("Katalog/SQLite-Datei wurde verändert".into());
                }
            } else if name == "catalog.sqlite3" {
                return Err("Bibliothekskatalog fehlt".into());
            }
        }
        let connection = self
            .connection
            .as_ref()
            .ok_or("Katalogreservierung wurde beendet")?;
        let mut moved = -1_i32;
        let result = unsafe {
            rusqlite::ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
                std::ptr::from_mut(&mut moved).cast(),
            )
        };
        if result != rusqlite::ffi::SQLITE_OK || moved != 0 {
            return Err(
                "Der reservierte Bibliothekskatalog wurde ersetzt oder kann nicht gebunden werden"
                    .into(),
            );
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
impl Drop for ReadReservation {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            let _ = connection.execute_batch("ROLLBACK");
            drop(connection);
        }
        // Remaining fields drop only after rollback and SQLite connection destruction.
    }
}
#[cfg(all(test, target_os = "macos"))]
#[path = "recovery_tests.rs"]
mod tests;

#[cfg(target_os = "macos")]
#[path = "recovery_stage.rs"]
mod stage;

#[cfg(target_os = "macos")]
fn normal_read_only(path: &Path) -> Result<rusqlite::Connection> {
    let value = path.to_str().ok_or("Katalogpfad ist kein UTF-8")?;
    if value.len() > 4096 {
        return Err("Katalogpfad ist zu lang".into());
    }
    let mut uri = String::with_capacity(value.len() * 3 + 40);
    uri.push_str("file:");
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.".contains(&b) {
            uri.push(b as char)
        } else {
            use std::fmt::Write;
            write!(&mut uri, "%{b:02X}").unwrap();
        }
    }
    // Installed Unix SQLite readonly_shm prevents a WAL transition from creating
    // a source-side SHM before our final namespace/stamp checks can reject it.
    uri.push_str("?mode=ro&readonly_shm=1");
    rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RecoveryKind {
    Backup,
    Restore,
}
#[derive(Debug, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum RecoveryRequest {
    Backup {
        destination_parent: String,
    },
    Restore {
        artifact_root: String,
        destination_parent: String,
    },
}
impl RecoveryRequest {
    pub(crate) fn kind(&self) -> RecoveryKind {
        match self {
            Self::Backup { .. } => RecoveryKind::Backup,
            Self::Restore { .. } => RecoveryKind::Restore,
        }
    }
}
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryLimits {
    pub max_logical_bytes: u64,
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_relative_path_bytes: usize,
    pub max_component_bytes: usize,
    pub max_manifest_bytes: u64,
    pub max_retained_bytes: usize,
    pub preparation_deadline_ms: u64,
    pub run_deadline_ms: u64,
    pub chunk_bytes: usize,
}
impl Default for RecoveryLimits {
    fn default() -> Self {
        Self {
            max_logical_bytes: 2 * 1024_u64.pow(4),
            max_entries: 250000,
            max_depth: 64,
            max_relative_path_bytes: 4096,
            max_component_bytes: 255,
            max_manifest_bytes: 128 * 1024 * 1024,
            max_retained_bytes: 256 * 1024 * 1024,
            preparation_deadline_ms: 1800000,
            run_deadline_ms: 86400000,
            chunk_bytes: 65536,
        }
    }
}
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoverySummary {
    pub entries: usize,
    pub files: usize,
    pub directories: usize,
    pub logical_bytes: u64,
    pub external_reference_records: Option<u64>,
    pub external_coverage: &'static str,
}
#[derive(Debug, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RecoveryPreparation {
    Ready {
        request_id: String,
        kind: RecoveryKind,
        destination_path: String,
        summary: RecoverySummary,
        limits: RecoveryLimits,
    },
    Unavailable {
        request_id: String,
        kind: RecoveryKind,
        reason: &'static str,
        message: String,
    },
    Cancelled {
        request_id: String,
        kind: RecoveryKind,
    },
}
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryProgress {
    pub request_id: String,
    pub kind: RecoveryKind,
    pub phase: &'static str,
    pub entries_done: usize,
    pub entries_total: Option<usize>,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub elapsed_ms: u64,
    pub current_relative_path: Option<String>,
}
#[derive(Debug, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RecoveryOutcome {
    Published {
        request_id: String,
        kind: RecoveryKind,
        destination_path: String,
        summary: RecoverySummary,
        durability: &'static str,
        warning: Option<String>,
    },
    Cancelled {
        request_id: String,
        kind: RecoveryKind,
        cleanup: &'static str,
    },
    Failed {
        request_id: String,
        kind: RecoveryKind,
        reason: &'static str,
        message: String,
        cleanup: &'static str,
        residue_path: Option<String>,
    },
}
#[derive(Debug)]
pub(crate) struct RecoveryFault {
    pub reason: &'static str,
    pub message: String,
    pub cancelled: bool,
}
impl RecoveryFault {
    fn new(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            cancelled: false,
        }
    }
}
type Checked<T> = std::result::Result<T, RecoveryFault>;
#[cfg(target_os = "macos")]
#[derive(Debug)]
struct SourceEntry {
    path: String,
    stamp: Stamp,
    sha256: Option<String>,
}
pub(crate) struct PreparedRecovery {
    pub(crate) kind: RecoveryKind,
    pub(crate) summary: RecoverySummary,
    pub(crate) destination: std::path::PathBuf,
    pub(crate) limits: RecoveryLimits,
    #[cfg(target_os = "macos")]
    source: std::path::PathBuf,
    #[cfg(target_os = "macos")]
    artifact: Option<std::path::PathBuf>,
    #[cfg(target_os = "macos")]
    entries: Vec<SourceEntry>,
    #[cfg(target_os = "macos")]
    source_root: Stamp,
    #[cfg(target_os = "macos")]
    ancestors: Vec<(std::path::PathBuf, (u64, u64))>,
    #[cfg(target_os = "macos")]
    destination_ancestors: Vec<(std::path::PathBuf, (u64, u64))>,
    #[cfg(target_os = "macos")]
    manifest_stamp: Option<Stamp>,
    #[cfg(target_os = "macos")]
    retained: usize,
}
impl PreparedRecovery {
    pub(crate) fn reply(&self, id: &str) -> RecoveryPreparation {
        RecoveryPreparation::Ready {
            request_id: id.into(),
            kind: self.kind,
            destination_path: self.destination.to_string_lossy().into(),
            summary: self.summary.clone(),
            limits: self.limits.clone(),
        }
    }
}
pub(crate) fn prepare_recovery(
    root: Option<&Path>,
    request: RecoveryRequest,
    control: &ImportControl,
    id: &str,
    progress: impl FnMut(RecoveryProgress),
) -> Checked<PreparedRecovery> {
    prepare_with_limits(
        root,
        request,
        control,
        id,
        progress,
        RecoveryLimits::default(),
    )
}
fn prepare_with_limits(
    root: Option<&Path>,
    request: RecoveryRequest,
    control: &ImportControl,
    id: &str,
    mut progress: impl FnMut(RecoveryProgress),
    limits: RecoveryLimits,
) -> Checked<PreparedRecovery> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, request, control, id, progress, limits);
        Err(RecoveryFault::new(
            "unsupported",
            "Sicherung/Wiederherstellung ist derzeit nur für macOS qualifiziert",
        ))
    }
    #[cfg(target_os = "macos")]
    {
        let started = Instant::now();
        let work = Duration::from_millis(limits.preparation_deadline_ms);
        check(control, started, work)?;
        let kind = request.kind();
        if id.is_empty()
            || id.len() > 128
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(RecoveryFault::new(
                "unsupported",
                "Ungültige Vorgangskennung",
            ));
        }
        progress(RecoveryProgress {
            request_id: id.into(),
            kind,
            phase: "preparing",
            entries_done: 0,
            entries_total: None,
            bytes_done: 0,
            bytes_total: None,
            elapsed_ms: 0,
            current_relative_path: None,
        });
        let (source, artifact, parent) = match request {
            RecoveryRequest::Backup { destination_parent } => (
                root.ok_or_else(|| {
                    RecoveryFault::new("unsupported", "Bitte zuerst eine gültige Bibliothek öffnen")
                })?
                .to_owned(),
                None,
                PathBuf::from(destination_parent),
            ),
            RecoveryRequest::Restore {
                artifact_root,
                destination_parent,
            } => {
                return prepare_restore(
                    PathBuf::from(artifact_root),
                    PathBuf::from(destination_parent),
                    id,
                    root,
                    control,
                    started,
                    work,
                    limits,
                )
            }
        };
        let ancestors = pins(&source)?;
        let destination_ancestors = pins(&parent)?;
        if parent.starts_with(&source) || root.is_some_and(|r| parent.starts_with(r)) {
            return Err(RecoveryFault::new(
                "destinationChanged",
                "Zielordner muss außerhalb der Quelle und aktiven Bibliothek liegen",
            ));
        }
        let leaf = format!(
            "library-{}-{id}",
            if kind == RecoveryKind::Backup {
                "backup"
            } else {
                "restored"
            }
        );
        let destination = parent.join(leaf);
        let directory = anchored::root(&parent).map_err(fault("destinationChanged"))?;
        if anchored::optional_child_stamp(&directory, destination.file_name().unwrap())
            .map_err(fault("destinationChanged"))?
            .is_some()
        {
            return Err(RecoveryFault::new(
                "destinationChanged",
                "Ziel existiert bereits",
            ));
        }
        let reader = ReadReservation::open(&source, control, started, work)?;
        let source_root = Stamp::read(
            &reader
                .reader
                .directory
                .metadata()
                .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
        );
        let (entries, retained) =
            inventory(&reader.reader.directory, control, started, work, &limits)?;
        let external = validate_catalog(&reader, &entries, control, started, work, &limits)?;
        reader.validate().map_err(fault("sourceChanged"))?;
        validate_pins(&ancestors, "sourceChanged")?;
        validate_pins(&destination_ancestors, "destinationChanged")?;
        check(control, started, work)?;
        let retained = plan_retained(
            retained,
            &ancestors,
            &destination_ancestors,
            &source,
            artifact.as_deref(),
            &limits,
        )?;
        let summary = summary(&entries, Some(external));
        drop(reader);
        Ok(PreparedRecovery {
            kind,
            summary,
            destination,
            limits,
            source,
            artifact,
            entries,
            source_root,
            ancestors,
            destination_ancestors,
            manifest_stamp: None,
            retained,
        })
    }
}
#[cfg(target_os = "macos")]
use std::{
    os::unix::fs::FileExt,
    path::{Component, PathBuf},
};
fn fault(reason: &'static str) -> impl FnOnce(String) -> RecoveryFault {
    move |message| RecoveryFault::new(reason, message)
}
fn check(control: &ImportControl, started: Instant, work: Duration) -> Checked<()> {
    if control.cancellation_flag().load(Ordering::Relaxed) {
        return Err(RecoveryFault {
            reason: "stale",
            message: "Vorgang wurde abgebrochen".into(),
            cancelled: true,
        });
    }
    if started.elapsed() >= work {
        return Err(RecoveryFault::new(
            "limit",
            "Zeitlimit überschritten; bitte neu vorbereiten",
        ));
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn path_valid(path: &str, limits: &RecoveryLimits) -> Checked<()> {
    if path.is_empty()
        || path.len() > limits.max_relative_path_bytes
        || path.split('/').count() > limits.max_depth
        || path.split('/').any(|c| {
            c.is_empty()
                || c == "."
                || c == ".."
                || c.len() > limits.max_component_bytes
                || c.contains(['\\', '\0'])
        })
        || !Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(RecoveryFault::new(
            "unsupported",
            "Ungültiger oder zu langer relativer Pfad",
        ));
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn pins(path: &Path) -> Checked<Vec<(PathBuf, (u64, u64))>> {
    if !path.is_absolute() || path.to_str().is_none() || path.as_os_str().len() > 4096 {
        return Err(RecoveryFault::new(
            "unsupported",
            "Absoluter UTF-8-Pfad benötigt",
        ));
    }
    let mut result = Vec::new();
    let mut current = PathBuf::from("/");
    for c in path.components() {
        match c {
            Component::Normal(n) => current.push(n),
            Component::RootDir => {}
            _ => return Err(RecoveryFault::new("unsupported", "Unsicherer Wurzelpfad")),
        }
        if result.len() >= 65 {
            return Err(RecoveryFault::new("limit", "Wurzelpfad ist zu tief"));
        }
        let m =
            fs::symlink_metadata(&current).map_err(|e| RecoveryFault::new("io", e.to_string()))?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(RecoveryFault::new(
                "unsupported",
                "Verknüpfte oder spezielle Elternordner sind nicht verfügbar",
            ));
        }
        result.push((current.clone(), Stamp::read(&m).identity()));
    }
    Ok(result)
}
#[cfg(target_os = "macos")]
fn validate_pins(pins: &[(PathBuf, (u64, u64))], reason: &'static str) -> Checked<()> {
    for (p, id) in pins {
        let m = fs::symlink_metadata(p).map_err(|e| RecoveryFault::new(reason, e.to_string()))?;
        if !m.is_dir() || m.file_type().is_symlink() || Stamp::read(&m).identity() != *id {
            return Err(RecoveryFault::new(reason, "Ein Elternordner wurde ersetzt"));
        }
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn inventory(
    root: &fs::File,
    control: &ImportControl,
    started: Instant,
    work: Duration,
    limits: &RecoveryLimits,
) -> Checked<(Vec<SourceEntry>, usize)> {
    let device = Stamp::read(
        &root
            .metadata()
            .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
    )
    .identity()
    .0;
    let mut out = Vec::new();
    let mut retained = 65536_usize;
    let mut bytes = 0_u64;
    fn walk(
        dir: &fs::File,
        prefix: &str,
        device: u64,
        out: &mut Vec<SourceEntry>,
        retained: &mut usize,
        bytes: &mut u64,
        control: &ImportControl,
        started: Instant,
        work: Duration,
        limits: &RecoveryLimits,
    ) -> Checked<()> {
        check(control, started, work)?;
        let before = Stamp::read(
            &dir.metadata()
                .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
        );
        let listing = anchored::Listing::new(dir).map_err(fault("io"))?;
        while let Some(name) = listing.next().map_err(fault("io"))? {
            check(control, started, work)?;
            let name = name
                .to_str()
                .ok_or_else(|| RecoveryFault::new("unsupported", "Dateiname ist kein UTF-8"))?;
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            path_valid(&path, limits)?;
            if out.len() >= limits.max_entries {
                return Err(RecoveryFault::new(
                    "limit",
                    "Datei-/Ordneranzahl überschritten",
                ));
            }
            let charge = path
                .len()
                .checked_mul(4)
                .and_then(|n| n.checked_add(1024))
                .ok_or_else(|| RecoveryFault::new("limit", "Inventarlimit überschritten"))?;
            *retained = retained
                .checked_add(charge)
                .ok_or_else(|| RecoveryFault::new("limit", "Inventarlimit überschritten"))?;
            // Reserve room for bounded manifest parser scratch and the independent output ledger.
            if *retained > limits.max_retained_bytes / 4 {
                return Err(RecoveryFault::new(
                    "limit",
                    "Inventar-Speicherlimit überschritten",
                ));
            }
            let stamp =
                anchored::child_stamp(dir, OsStr::new(name)).map_err(fault("unsupported"))?;
            if stamp.is_dir()
                && (name.starts_with(".import-")
                    || name.starts_with(".game-assets-sprite-")
                    || name.starts_with(".game-assets-recovery-"))
            {
                return Err(RecoveryFault::new(
                    "unsupported",
                    "Unvollständiger Import-/Export-/Sicherungsordner vorhanden",
                ));
            }
            if stamp.identity().0 != device || (!stamp.is_file() && !stamp.is_dir()) {
                return Err(RecoveryFault::new(
                    "unsupported",
                    "Spezielle Dateien oder Volume-Wechsel sind nicht verfügbar",
                ));
            }
            if stamp.is_file() {
                *bytes = bytes
                    .checked_add(stamp.size)
                    .ok_or_else(|| RecoveryFault::new("limit", "Größenlimit überschritten"))?;
                if *bytes > limits.max_logical_bytes {
                    return Err(RecoveryFault::new(
                        "limit",
                        "Logisches Größenlimit überschritten",
                    ));
                }
            }
            out.push(SourceEntry {
                path: path.clone(),
                stamp,
                sha256: None,
            });
            if stamp.is_dir() {
                let child =
                    anchored::directory(dir, OsStr::new(name)).map_err(fault("sourceChanged"))?;
                if Stamp::read(
                    &child
                        .metadata()
                        .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
                ) != stamp
                {
                    return Err(RecoveryFault::new(
                        "sourceChanged",
                        "Ordner wurde beim Öffnen ersetzt",
                    ));
                }
                walk(
                    &child, &path, device, out, retained, bytes, control, started, work, limits,
                )?;
            }
        }
        if Stamp::read(
            &dir.metadata()
                .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
        ) != before
        {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Ordnerinhalt wurde während der Vorbereitung verändert",
            ));
        }
        Ok(())
    }
    walk(
        root,
        "",
        device,
        &mut out,
        &mut retained,
        &mut bytes,
        control,
        started,
        work,
        limits,
    )?;
    out.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok((out, retained))
}
#[cfg(target_os = "macos")]
fn validate_catalog(
    source: &ReadReservation,
    entries: &[SourceEntry],
    control: &ImportControl,
    started: Instant,
    work: Duration,
    limits: &RecoveryLimits,
) -> Checked<u64> {
    check(control, started, work)?;
    source.validate().map_err(fault("sourceChanged"))?;
    let c = source.connection.as_ref().unwrap();
    let _budget = QueryBudget::install(c, control.cancellation_flag(), started, work);
    let version: i64 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| sql_fault(e, control, started, work))?;
    if version != 1 {
        return Err(RecoveryFault::new(
            "unsupported",
            "Katalogversion wird ohne Migration nicht unterstützt",
        ));
    }
    // This private recovery profile follows the current create/open schema without migrating it.
    type Column = (&'static str, &'static str, i64, i64, Option<&'static str>);
    const PACKAGES: &[Column] = &[
        ("id", "INTEGER", 0, 1, None),
        ("name", "TEXT", 1, 0, None),
        ("relative_path", "TEXT", 1, 0, None),
        ("source_path", "TEXT", 1, 0, None),
    ];
    const ASSETS: &[Column] = &[
        ("id", "INTEGER", 0, 1, None),
        ("package_id", "INTEGER", 1, 0, None),
        ("name", "TEXT", 1, 0, None),
        ("original_folder", "TEXT", 1, 0, None),
    ];
    const FILES: &[Column] = &[
        ("id", "INTEGER", 0, 1, None),
        ("asset_id", "INTEGER", 1, 0, None),
        ("original_filename", "TEXT", 1, 0, None),
        ("relative_path", "TEXT", 1, 0, None),
        ("format", "TEXT", 1, 0, None),
        ("size_bytes", "INTEGER", 1, 0, None),
    ];
    const SCHEMA: [(&str, &str, &[Column]); 14] = [
        ("asset_files", "table", FILES),
        (
            "asset_tags",
            "table",
            &[
                ("asset_id", "INTEGER", 1, 1, None),
                ("tag", "TEXT", 1, 2, None),
            ],
        ),
        ("assets", "table", ASSETS),
        (
            "audio_metadata",
            "table",
            &[
                ("file_id", "INTEGER", 0, 1, None),
                ("duration_seconds", "REAL", 0, 0, None),
                ("sample_rate", "INTEGER", 0, 0, None),
                ("channels", "INTEGER", 0, 0, None),
                ("codec", "TEXT", 0, 0, None),
                ("error", "TEXT", 0, 0, None),
            ],
        ),
        (
            "external_files",
            "table",
            &[
                ("file_id", "INTEGER", 0, 1, None),
                ("identity", "TEXT", 1, 0, None),
                ("modified", "TEXT", 1, 0, None),
            ],
        ),
        (
            "external_sources",
            "table",
            &[
                ("package_id", "INTEGER", 0, 1, None),
                ("root", "TEXT", 1, 0, None),
                ("identity", "TEXT", 1, 0, None),
            ],
        ),
        (
            "folder_assets",
            "table",
            &[
                ("folder_id", "INTEGER", 1, 1, None),
                ("asset_id", "INTEGER", 1, 2, None),
            ],
        ),
        (
            "folders",
            "table",
            &[
                ("id", "INTEGER", 0, 1, None),
                ("name", "TEXT", 1, 0, None),
                ("parent_id", "INTEGER", 0, 0, None),
            ],
        ),
        (
            "package_tags",
            "table",
            &[
                ("package_id", "INTEGER", 1, 1, None),
                ("tag", "TEXT", 1, 2, None),
            ],
        ),
        ("packages", "table", PACKAGES),
        (
            "trash",
            "table",
            &[
                ("id", "INTEGER", 0, 1, None),
                ("package_id", "INTEGER", 0, 0, None),
                ("file_id", "INTEGER", 0, 0, None),
                ("deleted_at", "INTEGER", 1, 0, Some("unixepoch()")),
            ],
        ),
        ("active_packages", "view", PACKAGES),
        ("active_asset_files", "view", FILES),
        ("active_assets", "view", ASSETS),
    ];
    let count:i64=c.query_row("SELECT count(*) FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0)).map_err(|e|sql_fault(e,control,started,work))?;
    if count != 14 {
        return Err(RecoveryFault::new(
            "unsupported",
            "Katalogschema ist nicht vollständig qualifiziert",
        ));
    }
    for (name, kind, columns) in SCHEMA {
        check(control, started, work)?;
        let valid: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name=?1 AND type=?2",
                [name, kind],
                |r| r.get(0),
            )
            .map_err(|e| sql_fault(e, control, started, work))?;
        if valid != 1 {
            return Err(RecoveryFault::new(
                "unsupported",
                "Erforderliches Katalogschema fehlt",
            ));
        }
        let unsupported = || {
            RecoveryFault::new(
                "unsupported",
                "Katalogspalten werden ohne Migration nicht unterstützt",
            )
        };
        let mut statement = c.prepare("SELECT cid,name,type,\"notnull\",dflt_value,pk,hidden FROM pragma_table_xinfo(?1) ORDER BY cid LIMIT ?2")
            .map_err(|e| sql_fault(e, control, started, work))?;
        let mut rows = statement
            .query(rusqlite::params![name, columns.len() as i64 + 1])
            .map_err(|e| sql_fault(e, control, started, work))?;
        let mut index = 0;
        while let Some(row) = rows
            .next()
            .map_err(|e| sql_fault(e, control, started, work))?
        {
            check(control, started, work)?;
            let Some((column_name, column_type, not_null, primary_key, default)) =
                columns.get(index)
            else {
                return Err(unsupported());
            };
            let actual_name = row.get_ref(1).map_err(|_| unsupported())?;
            let actual_type = row.get_ref(2).map_err(|_| unsupported())?;
            let actual_default = row.get_ref(4).map_err(|_| unsupported())?;
            let actual_default = if matches!(actual_default, rusqlite::types::ValueRef::Null) {
                None
            } else {
                Some(actual_default.as_str().map_err(|_| unsupported())?)
            };
            let (not_null, primary_key, default) = if kind == "view" {
                (0, 0, None)
            } else {
                (*not_null, *primary_key, *default)
            };
            if row.get::<_, i64>(0).map_err(|_| unsupported())? != index as i64
                || actual_name.as_str().map_err(|_| unsupported())? != *column_name
                || !actual_type
                    .as_str()
                    .map_err(|_| unsupported())?
                    .eq_ignore_ascii_case(column_type)
                || row.get::<_, i64>(3).map_err(|_| unsupported())? != not_null
                || actual_default != default
                || row.get::<_, i64>(5).map_err(|_| unsupported())? != primary_key
                || row.get::<_, i64>(6).map_err(|_| unsupported())? != 0
            {
                return Err(unsupported());
            }
            index += 1;
        }
        if index != columns.len() {
            return Err(unsupported());
        }
    }
    let integrity: String = c
        .query_row("PRAGMA integrity_check(1)", [], |r| r.get(0))
        .map_err(|e| sql_fault(e, control, started, work))?;
    if integrity != "ok" {
        return Err(RecoveryFault::new(
            "invalidArtifact",
            "Katalogintegrität ist beschädigt",
        ));
    }
    let mut fk = c
        .prepare("SELECT 1 FROM pragma_foreign_key_check LIMIT 1")
        .map_err(|e| sql_fault(e, control, started, work))?;
    if fk
        .query([])
        .map_err(|e| sql_fault(e, control, started, work))?
        .next()
        .map_err(|e| sql_fault(e, control, started, work))?
        .is_some()
    {
        return Err(RecoveryFault::new(
            "invalidArtifact",
            "Katalog enthält beschädigte Verknüpfungen",
        ));
    }
    let external: i64 = c
        .query_row("SELECT count(*) FROM external_sources", [], |r| r.get(0))
        .map_err(|e| sql_fault(e, control, started, work))?;
    if external < 0 || external as u64 > limits.max_entries as u64 {
        return Err(RecoveryFault::new("limit", "Referenzanzahl überschritten"));
    }
    let mut statement=c.prepare("SELECT f.relative_path,f.size_bytes FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE NOT EXISTS(SELECT 1 FROM external_sources es WHERE es.package_id=a.package_id)").map_err(|e|sql_fault(e,control,started,work))?;
    let mut rows = statement
        .query([])
        .map_err(|e| sql_fault(e, control, started, work))?;
    let mut n = 0;
    while let Some(row) = rows
        .next()
        .map_err(|e| sql_fault(e, control, started, work))?
    {
        check(control, started, work)?;
        if n >= limits.max_entries {
            return Err(RecoveryFault::new(
                "limit",
                "Katalog-Dateianzahl überschritten",
            ));
        }
        n += 1;
        let path: &str = row
            .get_ref(0)
            .map_err(|e| sql_fault(e, control, started, work))?
            .as_str()
            .map_err(|e| sql_fault(e.into(), control, started, work))?;
        path_valid(path, limits)?;
        let size: i64 = row
            .get(1)
            .map_err(|e| sql_fault(e, control, started, work))?;
        let index = entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .map_err(|_| {
                RecoveryFault::new(
                    "sourceChanged",
                    "Verwaltete Originaldatei fehlt (einschließlich Papierkorb)",
                )
            })?;
        if size < 0 || !entries[index].stamp.is_file() || entries[index].stamp.size != size as u64 {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Verwaltete Dateigröße stimmt nicht mit dem Katalog überein",
            ));
        }
    }
    source.validate().map_err(fault("sourceChanged"))?;
    check(control, started, work)?;
    Ok(external as u64)
}
#[cfg(target_os = "macos")]
fn summary(entries: &[SourceEntry], external: Option<u64>) -> RecoverySummary {
    let files = entries.iter().filter(|e| e.stamp.is_file()).count();
    RecoverySummary {
        entries: entries.len(),
        files,
        directories: entries.len() - files,
        logical_bytes: entries
            .iter()
            .filter(|e| e.stamp.is_file())
            .map(|e| e.stamp.size)
            .sum(),
        external_reference_records: external,
        external_coverage: "metadataOnly",
    }
}
#[cfg(target_os = "macos")]
#[path = "recovery_manifest.rs"]
mod manifest;
#[cfg(target_os = "macos")]
enum Input<'a> {
    Held(&'a fs::File),
    Owned(fs::File),
}
#[cfg(target_os = "macos")]
impl Input<'_> {
    fn file(&self) -> &fs::File {
        match self {
            Self::Held(f) => f,
            Self::Owned(f) => f,
        }
    }
}
#[cfg(target_os = "macos")]
fn parent_directory<'a>(root: &fs::File, path: &'a str) -> Checked<(fs::File, &'a str)> {
    let (prefix, leaf) = path.rsplit_once('/').unwrap_or(("", path));
    let mut dir = root
        .try_clone()
        .map_err(|e| RecoveryFault::new("io", e.to_string()))?;
    for part in prefix.split('/').filter(|p| !p.is_empty()) {
        let stamp =
            anchored::child_stamp(&dir, OsStr::new(part)).map_err(fault("sourceChanged"))?;
        if !stamp.is_dir() {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Übergeordneter Pfad ist kein Ordner",
            ));
        }
        dir = anchored::directory(&dir, OsStr::new(part)).map_err(fault("sourceChanged"))?;
        if Stamp::read(
            &dir.metadata()
                .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
        ) != stamp
        {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Elternordner wurde ersetzt",
            ));
        }
    }
    Ok((dir, leaf))
}
#[cfg(target_os = "macos")]
impl ReadReservation {
    fn input<'a>(&'a mut self, entry: &SourceEntry) -> Checked<Input<'a>> {
        self.validate().map_err(fault("sourceChanged"))?;
        let (dir, leaf) = parent_directory(&self.reader.directory, &entry.path)?;
        if anchored::child_stamp(&dir, OsStr::new(leaf)).map_err(fault("sourceChanged"))?
            != entry.stamp
        {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Vorbereitete Quelldatei wurde verändert",
            ));
        }
        if entry.path == "catalog.sqlite3" {
            return Ok(Input::Held(self.reader.catalog_file()));
        }
        if entry.path == "catalog.sqlite3-journal" {
            return self
                .reader
                .journal_file()
                .map(Input::Held)
                .ok_or_else(|| RecoveryFault::new("sourceChanged", "Vorbereitetes Journal fehlt"));
        }
        if self
            .protected
            .iter()
            .any(|s| s.identity() == entry.stamp.identity())
        {
            return Err(RecoveryFault::new(
                "unsupported",
                "Quelldatei ist mit einer SQLite-Datei verknüpft",
            ));
        }
        let f = anchored::child(&dir, OsStr::new(leaf)).map_err(fault("sourceChanged"))?;
        let stamp = match f.metadata() {
            Ok(m) => Stamp::read(&m),
            Err(e) => {
                self.retained_aliases.push(f);
                return Err(RecoveryFault::new("io", e.to_string()));
            }
        };
        if self
            .protected
            .iter()
            .any(|s| s.identity() == stamp.identity())
        {
            self.retained_aliases.push(f);
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Geöffnete Quelldatei wurde durch eine SQLite-Verknüpfung ersetzt",
            ));
        }
        if stamp != entry.stamp {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Geöffnete Quelldatei wurde ersetzt",
            ));
        }
        Ok(Input::Owned(f))
    }
    fn output(&mut self, root: &fs::File, path: &str) -> Checked<fs::File> {
        let (dir, leaf) = parent_directory(root, path)?;
        let expected =
            anchored::child_stamp(&dir, OsStr::new(leaf)).map_err(fault("sourceChanged"))?;
        if !expected.is_file()
            || expected.links() != 1
            || self
                .protected
                .iter()
                .any(|s| s.identity() == expected.identity())
        {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Ausgabedatei wurde durch eine Verknüpfung ersetzt",
            ));
        }
        let f = anchored::child(&dir, OsStr::new(leaf)).map_err(fault("sourceChanged"))?;
        let actual = match f.metadata() {
            Ok(m) => Stamp::read(&m),
            Err(e) => {
                self.retained_aliases.push(f);
                return Err(RecoveryFault::new("io", e.to_string()));
            }
        };
        if self
            .protected
            .iter()
            .any(|s| s.identity() == actual.identity())
        {
            self.retained_aliases.push(f);
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Geöffnete Ausgabe ist mit dem Quellkatalog verknüpft",
            ));
        }
        if actual != expected {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Ausgabedatei wurde beim Öffnen ersetzt",
            ));
        }
        Ok(f)
    }
    fn release(mut self) -> Option<String> {
        let c = self.connection.take()?;
        let rollback = c.execute_batch("ROLLBACK").err().map(|e| e.to_string());
        let close = c.close().err().map(|(_, e)| e.to_string());
        rollback.or(close)
    }
}
#[cfg(target_os = "macos")]
struct Work<'a> {
    control: &'a ImportControl,
    started: Instant,
    duration: Duration,
    read: u64,
    max_read: u64,
    id: &'a str,
    kind: RecoveryKind,
    progress: &'a mut dyn FnMut(RecoveryProgress),
    last: Instant,
    phase: &'static str,
    done: u64,
    entries: usize,
    total: u64,
    total_entries: usize,
}
#[cfg(target_os = "macos")]
impl Work<'_> {
    fn phase(&mut self, phase: &'static str) {
        self.phase = phase;
        self.done = 0;
        self.entries = 0;
        self.emit(None, true)
    }
    fn emit(&mut self, path: Option<&str>, force: bool) {
        if force || self.last.elapsed() >= Duration::from_millis(100) {
            (self.progress)(RecoveryProgress {
                request_id: self.id.into(),
                kind: self.kind,
                phase: self.phase,
                entries_done: self.entries,
                entries_total: Some(self.total_entries),
                bytes_done: self.done,
                bytes_total: Some(self.total),
                elapsed_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                current_relative_path: path.map(str::to_owned),
            });
            self.last = Instant::now()
        }
    }
    fn hash(
        &mut self,
        file: &fs::File,
        size: u64,
        mut output: Option<&mut fs::File>,
        path: &str,
    ) -> Checked<String> {
        use std::io::Write;
        let mut sha = super::duplicates::sdk::Sha256::new();
        let mut offset = 0_u64;
        let mut buffer = [0_u8; 65536];
        while offset < size {
            check(self.control, self.started, self.duration)?;
            let requested = (size - offset).min(buffer.len() as u64) as usize;
            self.read = self
                .read
                .checked_add(requested as u64)
                .ok_or_else(|| RecoveryFault::new("limit", "Lese-Arbeitslimit überschritten"))?;
            if self.read > self.max_read {
                return Err(RecoveryFault::new(
                    "limit",
                    "Lese-Arbeitslimit überschritten",
                ));
            }
            let n = file
                .read_at(&mut buffer[..requested], offset)
                .map_err(|e| RecoveryFault::new("io", e.to_string()))?;
            if n != requested {
                return Err(RecoveryFault::new(
                    "sourceChanged",
                    "Quelldatei ist unvollständig oder verändert",
                ));
            }
            sha.update(&buffer[..n]);
            if let Some(f) = output.as_mut() {
                f.write_all(&buffer[..n])
                    .map_err(|e| RecoveryFault::new("io", e.to_string()))?
            }
            offset += n as u64;
            self.done += n as u64;
            self.emit(Some(path), false);
        }
        check(self.control, self.started, self.duration)?;
        let actual = file
            .metadata()
            .map_err(|e| RecoveryFault::new("io", e.to_string()))?;
        if actual.len() != size {
            return Err(RecoveryFault::new(
                "sourceChanged",
                "Dateigröße wurde während des Lesens verändert",
            ));
        }
        let mut value = String::with_capacity(64);
        use std::fmt::Write as _;
        for b in sha.finish() {
            write!(&mut value, "{b:02x}").unwrap()
        }
        Ok(value)
    }
}
#[cfg(target_os = "macos")]
fn verify_inventory(
    root: &fs::File,
    plan: &PreparedRecovery,
    control: &ImportControl,
    started: Instant,
    duration: Duration,
) -> Checked<()> {
    let current = Stamp::read(
        &root
            .metadata()
            .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
    );
    if current != plan.source_root {
        return Err(RecoveryFault::new(
            "sourceChanged",
            "Vorbereiteter Quellordner wurde verändert",
        ));
    }
    fn walk(
        root: &fs::File,
        prefix: &str,
        plan: &PreparedRecovery,
        control: &ImportControl,
        started: Instant,
        duration: Duration,
        count: &mut usize,
    ) -> Checked<()> {
        let listing = anchored::Listing::new(root).map_err(fault("io"))?;
        while let Some(name) = listing.next().map_err(fault("io"))? {
            check(control, started, duration)?;
            let name = name
                .to_str()
                .ok_or_else(|| RecoveryFault::new("sourceChanged", "Dateiname wurde verändert"))?;
            let path = if prefix.is_empty() {
                name.into()
            } else {
                format!("{prefix}/{name}")
            };
            path_valid(&path, &plan.limits)?;
            let i = plan
                .entries
                .binary_search_by(|e| e.path.cmp(&path))
                .map_err(|_| {
                    RecoveryFault::new(
                        "sourceChanged",
                        "Unbekannte Datei/Ordner zur Quelle hinzugefügt",
                    )
                })?;
            let current =
                anchored::child_stamp(root, OsStr::new(name)).map_err(fault("sourceChanged"))?;
            if current != plan.entries[i].stamp {
                return Err(RecoveryFault::new(
                    "sourceChanged",
                    "Vorbereitete Datei oder Ordner wurde ersetzt/verändert",
                ));
            }
            *count += 1;
            if *count > plan.entries.len() {
                return Err(RecoveryFault::new(
                    "sourceChanged",
                    "Quelltopologie wurde verändert",
                ));
            }
            if current.is_dir() {
                let dir =
                    anchored::directory(root, OsStr::new(name)).map_err(fault("sourceChanged"))?;
                walk(&dir, &path, plan, control, started, duration, count)?
            }
        }
        Ok(())
    }
    let mut count = 0;
    walk(root, "", plan, control, started, duration, &mut count)?;
    if count != plan.entries.len() {
        return Err(RecoveryFault::new(
            "sourceChanged",
            "Vorbereitete Quelle ist unvollständig",
        ));
    }
    validate_pins(&plan.ancestors, "sourceChanged")?;
    Ok(())
}
pub(crate) type RecoveryPublisher<'a> =
    dyn FnMut(&mut dyn FnMut() -> Result<Option<String>>) -> Result<Option<String>> + 'a;
pub(crate) fn run_recovery(
    mut plan: PreparedRecovery,
    control: &ImportControl,
    id: &str,
    mut progress: impl FnMut(RecoveryProgress),
    publisher: &mut RecoveryPublisher<'_>,
) -> RecoveryOutcome {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&mut plan, control, progress, publisher);
        RecoveryOutcome::Failed {
            request_id: id.into(),
            kind: plan.kind,
            reason: "unsupported",
            message: "Sicherung/Wiederherstellung ist derzeit nur für macOS qualifiziert".into(),
            cleanup: "notNeeded",
            residue_path: None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        let started = Instant::now();
        let duration = Duration::from_millis(plan.limits.run_deadline_ms);
        let kind = plan.kind;
        let mut stage: Option<stage::Stage<'_>> = None;
        let mut source: Option<ReadReservation> = None;
        let operation = (|| -> Checked<Option<String>> {
            let mut work = Work {
                control,
                started,
                duration,
                read: 0,
                max_read: plan.summary.logical_bytes.checked_mul(3).ok_or_else(|| {
                    RecoveryFault::new("limit", "Lese-Arbeitslimit überschritten")
                })?,
                id,
                kind,
                progress: &mut progress,
                last: started,
                phase: "reserving",
                done: 0,
                entries: 0,
                total: plan.summary.logical_bytes,
                total_entries: plan.entries.len(),
            };
            work.phase("reserving");
            check(control, started, duration)?;
            validate_pins(&plan.ancestors, "sourceChanged")?;
            validate_pins(&plan.destination_ancestors, "destinationChanged")?;
            source = Some(ReadReservation::open(
                &plan.source,
                control,
                started,
                duration,
            )?);
            let reader = source.as_mut().unwrap();
            verify_inventory(&reader.reader.directory, &plan, control, started, duration)?;
            validate_catalog(
                reader,
                &plan.entries,
                control,
                started,
                duration,
                &plan.limits,
            )?;
            if plan.kind == RecoveryKind::Restore {
                validate_artifact_envelope(&plan, control, started, duration)?;
            }
            let parent = plan
                .destination
                .parent()
                .ok_or_else(|| RecoveryFault::new("destinationChanged", "Zielordner fehlt"))?;
            let leaf = plan
                .destination
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    RecoveryFault::new("destinationChanged", "Zielpfad ist kein UTF-8")
                })?;
            let ledger_budget = plan
                .limits
                .max_retained_bytes
                .checked_sub(plan.retained)
                .and_then(|n| n.checked_sub(plan.limits.max_manifest_bytes as usize))
                .ok_or_else(|| RecoveryFault::new("limit", "Retained-Datenlimit überschritten"))?;
            stage = Some(
                stage::Stage::create(
                    parent,
                    leaf,
                    control,
                    plan.limits.max_entries.checked_add(3).ok_or_else(|| {
                        RecoveryFault::new("limit", "Ausgabe-Anzahllimit überschritten")
                    })?,
                    ledger_budget,
                )
                .map_err(fault("io"))?,
            );
            let stage = stage.as_mut().unwrap();
            let prefix = if kind == RecoveryKind::Backup {
                stage.create_dir("library").map_err(fault("io"))?;
                "library/"
            } else {
                ""
            };
            work.phase("copying");
            for entry in &mut plan.entries {
                check(control, started, duration)?;
                let output_path = format!("{prefix}{}", entry.path);
                if entry.stamp.is_dir() {
                    stage.create_dir(&output_path).map_err(fault("io"))?
                } else {
                    let input = reader.input(entry)?;
                    let mut output = stage.create_file(&output_path).map_err(fault("io"))?;
                    let hash = work.hash(
                        input.file(),
                        entry.stamp.size,
                        Some(&mut output),
                        &entry.path,
                    )?;
                    if Stamp::read(
                        &input
                            .file()
                            .metadata()
                            .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
                    ) != entry.stamp
                    {
                        return Err(RecoveryFault::new(
                            "sourceChanged",
                            "Quelle wurde während der Kopie verändert",
                        ));
                    }
                    if entry.sha256.as_ref().is_some_and(|h| h != &hash) {
                        return Err(RecoveryFault::new(
                            "invalidArtifact",
                            "Sicherungsdatei stimmt nicht mit ihrem SHA256 überein",
                        ));
                    }
                    entry.sha256 = Some(hash);
                    stage
                        .finish_file(&output_path, &output)
                        .map_err(fault("io"))?;
                }
                work.entries += 1;
                work.emit(Some(&entry.path), false);
            }
            work.phase("verifyingCopy");
            stage.validate().map_err(fault("sourceChanged"))?;
            for entry in &plan.entries {
                if entry.stamp.is_file() {
                    let path = format!("{prefix}{}", entry.path);
                    let output = reader.output(stage.directory(), &path)?;
                    let hash = work.hash(&output, entry.stamp.size, None, &entry.path)?;
                    if Some(&hash) != entry.sha256.as_ref() {
                        return Err(RecoveryFault::new(
                            "sourceChanged",
                            "Kopierte Datei stimmt nicht mit der Quelle überein",
                        ));
                    }
                }
                work.entries += 1;
                work.emit(Some(&entry.path), false)
            }
            stage.validate().map_err(fault("sourceChanged"))?;
            if kind == RecoveryKind::Backup {
                let manifest = manifest::Manifest {
                    format: "game-asset-library-backup".into(),
                    version: 1,
                    catalog_version: 1,
                    external_coverage: "metadataOnly".into(),
                    external_reference_records: plan
                        .summary
                        .external_reference_records
                        .unwrap_or(0),
                    entries: plan
                        .entries
                        .iter()
                        .map(|e| manifest::ManifestEntry {
                            path: e.path.clone(),
                            kind: if e.stamp.is_dir() {
                                manifest::EntryKind::Directory
                            } else {
                                manifest::EntryKind::File
                            },
                            bytes: if e.stamp.is_file() { e.stamp.size } else { 0 },
                            sha256: e.sha256.clone(),
                        })
                        .collect(),
                };
                let mut f = stage.create_file("manifest.json").map_err(fault("io"))?;
                manifest::write(
                    &mut f,
                    &manifest,
                    plan.limits.max_manifest_bytes,
                    control,
                    started,
                    duration,
                )
                .map_err(fault("limit"))?;
                stage
                    .finish_file("manifest.json", &f)
                    .map_err(fault("io"))?;
            }
            work.phase("validatingCatalog");
            let output_root = stage.staged_root().join(if kind == RecoveryKind::Backup {
                "library"
            } else {
                ""
            });
            let copied = ReadReservation::open(&output_root, control, started, duration)?;
            let external = validate_catalog(
                &copied,
                &plan.entries,
                control,
                started,
                duration,
                &plan.limits,
            )?;
            if plan.summary.external_reference_records != Some(external) {
                return Err(RecoveryFault::new(
                    "invalidArtifact",
                    "Externe Katalogabdeckung wurde verändert",
                ));
            }
            drop(copied);
            work.phase("verifyingSource");
            for entry in &plan.entries {
                if entry.stamp.is_file() {
                    let input = reader.input(entry)?;
                    let hash = work.hash(input.file(), entry.stamp.size, None, &entry.path)?;
                    if Some(&hash) != entry.sha256.as_ref() {
                        return Err(RecoveryFault::new(
                            "sourceChanged",
                            "Originaldatei wurde während des Vorgangs verändert",
                        ));
                    }
                }
                work.entries += 1;
                work.emit(Some(&entry.path), false)
            }
            verify_inventory(&reader.reader.directory, &plan, control, started, duration)?;
            validate_catalog(
                reader,
                &plan.entries,
                control,
                started,
                duration,
                &plan.limits,
            )?;
            if kind == RecoveryKind::Restore {
                validate_artifact_envelope(&plan, control, started, duration)?;
            }
            stage.validate().map_err(fault("sourceChanged"))?;
            stage.sync().map_err(fault("io"))?;
            work.phase("publishing");
            check(control, started, duration)?;
            validate_pins(&plan.destination_ancestors, "destinationChanged")?;
            let mut publish = || {
                check(control, started, duration).map_err(|e| e.message)?;
                reader.validate()?;
                verify_inventory(&reader.reader.directory, &plan, control, started, duration)
                    .map_err(|e| e.message)?;
                if kind == RecoveryKind::Restore {
                    validate_artifact_envelope(&plan, control, started, duration)
                        .map_err(|e| e.message)?;
                }
                validate_pins(&plan.destination_ancestors, "destinationChanged")
                    .map_err(|e| e.message)?;
                stage.publish()
            };
            let warning = publisher(&mut publish).map_err(fault("sourceChanged"))?;
            Ok(warning)
        })();
        match operation {
            Ok(mut warning) => {
                if let Some(source) = source.take() {
                    if let Some(e) = source.release() {
                        warning = Some(format!(
                            "{}{}Katalogfreigabe nach Veröffentlichung unsicher: {e}",
                            warning.unwrap_or_default(),
                            " "
                        ))
                    }
                }
                RecoveryOutcome::Published {
                    request_id: id.into(),
                    kind,
                    destination_path: plan.destination.to_string_lossy().into(),
                    summary: plan.summary,
                    durability: if warning.is_some() {
                        "uncertain"
                    } else {
                        "synced"
                    },
                    warning,
                }
            }
            Err(error) if stage.as_ref().is_some_and(|s| s.published()) => {
                let mut message = error.message;
                if let Some(source) = source.take() {
                    if let Some(e) = source.release() {
                        message.push_str(&format!("; {e}"));
                    }
                }
                RecoveryOutcome::Published {
                    request_id: id.into(),
                    kind,
                    destination_path: plan.destination.to_string_lossy().into(),
                    summary: plan.summary,
                    durability: "uncertain",
                    warning: Some(message),
                }
            }
            Err(mut error) => {
                if error.message == "Manifest-Vorgang wurde abgebrochen" {
                    error.cancelled = true;
                }
                let residue = stage
                    .as_ref()
                    .map(|s| s.staged_root().to_string_lossy().into_owned());
                progress(RecoveryProgress {
                    request_id: id.into(),
                    kind,
                    phase: "cleaningUp",
                    entries_done: 0,
                    entries_total: None,
                    bytes_done: 0,
                    bytes_total: None,
                    elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    current_relative_path: None,
                });
                let cleanup =
                    stage.as_mut().is_none_or(|s| s.cleanup()) && !control.cleanup_failed();
                drop(stage);
                drop(source);
                if (error.cancelled
                    || error.message == "Wiederherstellungsvorgang wurde abgebrochen")
                    && cleanup
                {
                    RecoveryOutcome::Cancelled {
                        request_id: id.into(),
                        kind,
                        cleanup: "complete",
                    }
                } else {
                    RecoveryOutcome::Failed {
                        request_id: id.into(),
                        kind,
                        reason: if !cleanup {
                            "cleanupIncomplete"
                        } else {
                            error.reason
                        },
                        message: error.message,
                        cleanup: if !cleanup {
                            "retained"
                        } else if residue.is_some() {
                            "complete"
                        } else {
                            "notNeeded"
                        },
                        residue_path: if cleanup { None } else { residue },
                    }
                }
            }
        }
    }
}
#[cfg(target_os = "macos")]
fn validate_artifact_envelope(
    plan: &PreparedRecovery,
    control: &ImportControl,
    started: Instant,
    work: Duration,
) -> Checked<()> {
    check(control, started, work)?;
    let artifact = plan
        .artifact
        .as_ref()
        .ok_or_else(|| RecoveryFault::new("invalidArtifact", "Sicherungswurzel fehlt"))?;
    let directory = anchored::root(artifact).map_err(fault("sourceChanged"))?;
    let listing = anchored::Listing::new(&directory).map_err(fault("io"))?;
    let mut n = 0;
    while let Some(name) = listing.next().map_err(fault("io"))? {
        check(control, started, work)?;
        if name != OsStr::new("manifest.json") && name != OsStr::new("library") {
            return Err(RecoveryFault::new(
                "invalidArtifact",
                "Unbekannte Datei im Sicherungsartefakt",
            ));
        }
        n += 1;
        if n > 2 {
            return Err(RecoveryFault::new(
                "invalidArtifact",
                "Sicherungsartefakt enthält zusätzliche Dateien",
            ));
        }
    }
    if n != 2
        || anchored::child_stamp(&directory, OsStr::new("manifest.json"))
            .map_err(fault("sourceChanged"))?
            != plan
                .manifest_stamp
                .ok_or_else(|| RecoveryFault::new("invalidArtifact", "Manifest-Version fehlt"))?
    {
        return Err(RecoveryFault::new(
            "sourceChanged",
            "Manifest oder Artefakttopologie wurde verändert",
        ));
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn prepare_restore(
    artifact: PathBuf,
    parent: PathBuf,
    id: &str,
    root: Option<&Path>,
    control: &ImportControl,
    started: Instant,
    work: Duration,
    limits: RecoveryLimits,
) -> Checked<PreparedRecovery> {
    let ancestors = pins(&artifact)?;
    let source = artifact.join("library");
    let source_pins = pins(&source)?;
    let destination_ancestors = pins(&parent)?;
    if parent.starts_with(&artifact) || root.is_some_and(|r| parent.starts_with(r)) {
        return Err(RecoveryFault::new(
            "destinationChanged",
            "Zielordner muss außerhalb der Sicherung und aktiven Bibliothek liegen",
        ));
    }
    let artifact_dir = anchored::root(&artifact).map_err(fault("invalidArtifact"))?;
    let library_dir = anchored::directory(&artifact_dir, OsStr::new("library"))
        .map_err(fault("invalidArtifact"))?;
    // All known SQL inode aliases are refused before any regular manifest/catalogue open.
    let mut known = Vec::new();
    for dir in [&library_dir]
        .into_iter()
        .chain(root.and_then(|r| anchored::root(r).ok()).as_ref())
    {
        for leaf in [
            "catalog.sqlite3",
            "catalog.sqlite3-journal",
            "catalog.sqlite3-wal",
            "catalog.sqlite3-shm",
        ] {
            if let Some(stamp) = anchored::optional_child_stamp(dir, OsStr::new(leaf))
                .map_err(fault("unsupported"))?
            {
                if !stamp.is_file() || stamp.links() != 1 {
                    return Err(RecoveryFault::new(
                        "unsupported",
                        "Verknüpfte SQLite-Dateien sind nicht verfügbar",
                    ));
                }
                known.push(stamp.identity())
            }
        }
    }
    let expected = anchored::child_stamp(&artifact_dir, OsStr::new("manifest.json"))
        .map_err(fault("invalidArtifact"))?;
    if !expected.is_file()
        || expected.size > limits.max_manifest_bytes
        || known.contains(&expected.identity())
    {
        return Err(RecoveryFault::new(
            "invalidArtifact",
            "Manifest ist ungültig, zu groß oder mit SQLite verknüpft",
        ));
    }
    let file = anchored::child(&artifact_dir, OsStr::new("manifest.json"))
        .map_err(fault("invalidArtifact"))?;
    if Stamp::read(
        &file
            .metadata()
            .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
    ) != expected
    {
        return Err(RecoveryFault::new(
            "sourceChanged",
            "Manifest wurde beim Öffnen verändert",
        ));
    }
    let parsed = manifest::read(
        &file,
        limits.max_manifest_bytes,
        limits.max_entries,
        limits.max_retained_bytes,
        control,
        started,
        work,
    )
    .map_err(fault("invalidArtifact"))?;
    drop(file);
    let reader = ReadReservation::open(&source, control, started, work)?;
    let source_root = Stamp::read(
        &reader
            .reader
            .directory
            .metadata()
            .map_err(|e| RecoveryFault::new("io", e.to_string()))?,
    );
    let (mut entries, retained) =
        inventory(&reader.reader.directory, control, started, work, &limits)?;
    if entries.len() != parsed.entries.len() {
        return Err(RecoveryFault::new(
            "invalidArtifact",
            "Sicherungsdateien entsprechen nicht dem Manifest",
        ));
    }
    for (entry, record) in entries.iter_mut().zip(parsed.entries) {
        if entry.path != record.path
            || entry.stamp.is_dir() != (record.kind == manifest::EntryKind::Directory)
            || entry.stamp.is_file() && (entry.stamp.size != record.bytes)
        {
            return Err(RecoveryFault::new(
                "invalidArtifact",
                "Manifestpfad/-größe entspricht nicht der Sicherung",
            ));
        }
        entry.sha256 = record.sha256;
    }
    let external = validate_catalog(&reader, &entries, control, started, work, &limits)?;
    if external != parsed.external_reference_records {
        return Err(RecoveryFault::new(
            "invalidArtifact",
            "Externe Referenzabdeckung stimmt nicht mit dem Katalog überein",
        ));
    }
    let retained = plan_retained(
        retained,
        &source_pins,
        &destination_ancestors,
        &source,
        Some(&artifact),
        &limits,
    )?;
    let summary = summary(&entries, Some(external));
    drop(reader);
    validate_pins(&ancestors, "sourceChanged")?;
    validate_pins(&source_pins, "sourceChanged")?;
    validate_pins(&destination_ancestors, "destinationChanged")?;
    let destination = parent.join(format!("library-restored-{id}"));
    let directory = anchored::root(&parent).map_err(fault("destinationChanged"))?;
    if anchored::optional_child_stamp(&directory, destination.file_name().unwrap())
        .map_err(fault("destinationChanged"))?
        .is_some()
    {
        return Err(RecoveryFault::new(
            "destinationChanged",
            "Ziel existiert bereits",
        ));
    }
    let plan = PreparedRecovery {
        kind: RecoveryKind::Restore,
        summary,
        destination,
        limits,
        source,
        artifact: Some(artifact),
        entries,
        source_root,
        ancestors: source_pins,
        destination_ancestors,
        manifest_stamp: Some(expected),
        retained,
    };
    validate_artifact_envelope(&plan, control, started, work)?;
    check(control, started, work)?;
    Ok(plan)
}

fn sql_fault(
    error: rusqlite::Error,
    control: &ImportControl,
    started: Instant,
    work: Duration,
) -> RecoveryFault {
    if let Err(e) = check(control, started, work) {
        return e;
    }
    if matches!(&error,rusqlite::Error::SqliteFailure(e,_) if e.code==rusqlite::ErrorCode::DatabaseBusy || e.code==rusqlite::ErrorCode::DatabaseLocked)
    {
        return RecoveryFault::new("busy", error.to_string());
    }
    RecoveryFault::new("invalidArtifact", error.to_string())
}

impl RecoveryFault {
    pub(crate) fn preparation(
        self,
        id: &str,
        kind: RecoveryKind,
        control: &ImportControl,
    ) -> RecoveryPreparation {
        if self.cancelled || control.cancellation_flag().load(Ordering::Relaxed) {
            RecoveryPreparation::Cancelled {
                request_id: id.into(),
                kind,
            }
        } else {
            RecoveryPreparation::Unavailable {
                request_id: id.into(),
                kind,
                reason: self.reason,
                message: self.message,
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn plan_retained(
    inventory: usize,
    source_pins: &[(PathBuf, (u64, u64))],
    target_pins: &[(PathBuf, (u64, u64))],
    source: &Path,
    artifact: Option<&Path>,
    limits: &RecoveryLimits,
) -> Checked<usize> {
    let mut retained = inventory;
    for (path, _) in source_pins.iter().chain(target_pins.iter()) {
        retained = retained
            .checked_add(path.as_os_str().len() * 4 + 512)
            .ok_or_else(|| RecoveryFault::new("limit", "Plan-Speicherlimit überschritten"))?;
    }
    retained = retained
        .checked_add(
            source.as_os_str().len() * 4 + artifact.map_or(0, |p| p.as_os_str().len() * 4) + 32768,
        )
        .ok_or_else(|| RecoveryFault::new("limit", "Plan-Speicherlimit überschritten"))?;
    if retained > limits.max_retained_bytes / 4 {
        return Err(RecoveryFault::new(
            "limit",
            "Plan-Speicherlimit überschritten",
        ));
    }
    Ok(retained)
}
