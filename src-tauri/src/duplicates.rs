use super::Result;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DuplicateFile {
    pub file_id: i64,
    pub asset_id: i64,
    pub package_id: i64,
    pub package_name: String,
    pub original_filename: String,
    pub relative_path: String,
    pub format: String,
    pub size_bytes: u64,
    pub status: &'static str,
    pub sha256: Option<String>,
    pub reason: Option<String>,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DuplicateGroup {
    pub sha256: String,
    pub size_bytes: u64,
    pub members: Vec<i64>,
    pub shared_source: bool,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DuplicateReport {
    pub complete: bool,
    pub cancelled: bool,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub total: usize,
    pub processed: usize,
    pub bytes_read: u64,
    pub groups: Vec<DuplicateGroup>,
    pub files: Vec<DuplicateFile>,
    pub issues: Vec<String>,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DuplicateProgress {
    pub stage: &'static str,
    pub total: usize,
    pub processed: usize,
    pub bytes_read: u64,
}
#[derive(Clone, Copy)]
struct Limits {
    rows: usize,
    text: usize,
    file: u64,
    bytes: u64,
    work: Duration,
}
const LIMITS: Limits = Limits {
    rows: 5000,
    text: 16 * 1024 * 1024,
    file: 4 * 1024 * 1024 * 1024,
    bytes: 32 * 1024 * 1024 * 1024,
    work: Duration::from_secs(120),
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct External {
    root: PathBuf,
    root_identity: String,
    identity: String,
    modified: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub(super) file: DuplicateFile,
    pub(super) package_path: PathBuf,
    pub(super) external: Option<External>,
}
pub(crate) struct PreparedDuplicateCheck {
    #[cfg(target_os = "macos")]
    pub(super) reader: super::storage::CatalogReader,
    #[cfg(target_os = "macos")]
    pub(super) versions: Vec<Result<Version>>,
    pub(super) records: Vec<Record>,
    package_id: Option<i64>,
}
impl DuplicateReport {
    fn issue(&mut self, message: impl AsRef<str>) {
        if self.issues.len() < 30 {
            self.issues
                .push(message.as_ref().chars().take(300).collect());
        }
    }
}
impl PreparedDuplicateCheck {
    pub(crate) fn total(&self) -> usize {
        self.records.len()
    }
    #[cfg(target_os = "macos")]
    pub(super) fn retained_preimport_bytes(&self) -> usize {
        let record_bytes = self.records.capacity() * std::mem::size_of::<Record>()
            + self
                .records
                .iter()
                .map(|r| {
                    let f = &r.file;
                    f.package_name.capacity()
                        + f.original_filename.capacity()
                        + f.relative_path.capacity()
                        + f.format.capacity()
                        + r.package_path.capacity()
                        + r.external.as_ref().map_or(0, |e| {
                            e.root.capacity()
                                + e.root_identity.capacity()
                                + e.identity.capacity()
                                + e.modified.capacity()
                        })
                })
                .sum::<usize>();
        record_bytes
            + self.versions.capacity() * std::mem::size_of::<Result<Version>>()
            + self
                .versions
                .iter()
                .map(|v| match v {
                    Ok(v) => {
                        v.parents.capacity() * std::mem::size_of::<super::storage::Stamp>()
                            + v.physical.capacity()
                    }
                    Err(e) => e.capacity(),
                })
                .sum::<usize>()
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn normal_path(text: &str) -> Result<PathBuf> {
    let path = PathBuf::from(text);
    if path.as_os_str().is_empty()
        || path.components().count() > 48
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err("Ungültiger Variantenpfad".into());
    }
    Ok(path)
}
fn guard(flag: &AtomicBool, started: Instant, work: Duration) -> Result<()> {
    if flag.load(Ordering::Relaxed) {
        return Err("Duplikatprüfung abgebrochen".into());
    }
    if started.elapsed() >= work {
        return Err("Zeitlimit der Duplikatprüfung erreicht".into());
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn snapshot(
    reader: &super::storage::CatalogReader,
    package_id: Option<i64>,
    flag: &AtomicBool,
    limits: Limits,
    started: Instant,
) -> Result<Vec<Record>> {
    guard(flag, started, limits.work)?;
    const JS_SAFE: i64 = 9_007_199_254_740_991;
    let id = |value: i64| -> Result<i64> {
        if value <= 0 || value > JS_SAFE {
            return Err("Variantenkennung ist keine sichere positive Zahl".into());
        }
        Ok(value)
    };
    if let Some(package_id) = package_id {
        id(package_id)?;
    }
    let _vm = super::storage::QueryBudget::install(
        &reader.library.connection,
        flag,
        started,
        limits.work,
    );
    if let Some(package_id) = package_id {
        let active: bool = reader.library.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM packages p WHERE p.id=?1 AND NOT EXISTS(SELECT 1 FROM trash t WHERE t.package_id=p.id))",
            [package_id], |row| row.get(0),
        ).map_err(|e|e.to_string())?;
        if !active {
            return Err("Das ausgewählte Paket ist nicht mehr aktiv".into());
        }
    }
    let mut statement=reader.library.connection.prepare("SELECT f.id,a.id,p.id,p.name,f.original_filename,f.relative_path,f.format,f.size_bytes,p.relative_path,s.root,s.identity,e.identity,e.modified FROM active_asset_files f JOIN assets a ON a.id=f.asset_id JOIN packages p ON p.id=a.package_id LEFT JOIN external_sources s ON s.package_id=p.id LEFT JOIN external_files e ON e.file_id=f.id WHERE (?1 IS NULL OR p.id=?1) ORDER BY f.id LIMIT ?2").map_err(|e|e.to_string())?;
    let mut rows = statement
        .query(rusqlite::params![package_id, (limits.rows + 1) as i64])
        .map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    let mut retained = 0usize;
    let mut file_ids = HashSet::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        guard(flag, started, limits.work)?;
        if records.len() == limits.rows {
            return Err(
                "Mehr als 5.000 Dateivarianten; bitte ein kleineres Paket auswählen".into(),
            );
        }
        let file_id = id(row.get(0).map_err(|e| e.to_string())?)?;
        let asset_id = id(row.get(1).map_err(|e| e.to_string())?)?;
        let package_id = id(row.get(2).map_err(|e| e.to_string())?)?;
        if !file_ids.insert(file_id) {
            return Err("Variantenkennung ist mehrfach vorhanden".into());
        }
        let mut text = |column| -> Result<Option<String>> {
            let value = row.get_ref(column).map_err(|e| e.to_string())?;
            if value == rusqlite::types::ValueRef::Null {
                return Ok(None);
            }
            let value = value.as_str().map_err(|_| "Ungültiger Katalogtext")?;
            retained = retained
                .checked_add(value.len())
                .ok_or("Katalogtext zu groß")?;
            if value.len() > 4096 || retained > limits.text {
                return Err("Katalogtext überschreitet das Duplikatprüfung-Limit".into());
            }
            Ok(Some(value.into()))
        };
        let package_name = text(3)?.ok_or("Paketname fehlt")?;
        let filename = text(4)?.ok_or("Dateiname fehlt")?;
        let relative = text(5)?.ok_or("Dateipfad fehlt")?;
        let format = text(6)?.ok_or("Format fehlt")?;
        let package_path = normal_path(&text(8)?.ok_or("Paketpfad fehlt")?)?;
        let root = text(9)?;
        let root_identity = text(10)?;
        let identity = text(11)?;
        let modified = text(12)?;
        let external = match (root, root_identity, identity, modified) {
            (None, None, None, None) => None,
            (Some(root), Some(root_identity), Some(identity), Some(modified)) => Some(External {
                root: PathBuf::from(root),
                root_identity,
                identity,
                modified,
            }),
            _ => return Err("Externe Variantenidentität ist unvollständig".into()),
        };
        let expected = if external.is_some() {
            "external"
        } else {
            "packages"
        };
        if package_path.components().count() != 2
            || package_path.components().next().unwrap().as_os_str() != expected
            || !normal_path(&relative)?.starts_with(&package_path)
        {
            return Err("Variante gehört nicht zum Paketpfad".into());
        }
        let size: i64 = row.get(7).map_err(|e| e.to_string())?;
        let size_bytes = u64::try_from(size).map_err(|_| "Ungültige Variantengröße")?;
        if size > JS_SAFE {
            return Err("Variantengröße ist keine sichere Zahl".into());
        }
        records.push(Record {
            file: DuplicateFile {
                file_id,
                asset_id,
                package_id,
                package_name,
                original_filename: filename,
                relative_path: relative,
                format,
                size_bytes,
                status: "notChecked",
                sha256: None,
                reason: None,
            },
            package_path,
            external,
        });
    }
    Ok(records)
}
pub(crate) fn prepare_duplicate_check(
    root: &Path,
    package_id: Option<i64>,
    flag: &AtomicBool,
) -> Result<PreparedDuplicateCheck> {
    prepare_with_limits(root, package_id, flag, LIMITS, Instant::now())
}
pub(super) fn prepare_preimport_catalog(
    root: &Path,
    flag: &AtomicBool,
    started: Instant,
    work: Duration,
) -> Result<PreparedDuplicateCheck> {
    prepare_with_limits(root, None, flag, Limits { work, ..LIMITS }, started)
}
fn prepare_with_limits(
    root: &Path,
    package_id: Option<i64>,
    flag: &AtomicBool,
    limits: Limits,
    started: Instant,
) -> Result<PreparedDuplicateCheck> {
    #[cfg(target_os = "macos")]
    {
        guard(flag, started, limits.work)?;
        let reader = super::storage::CatalogReader::open(root)?;
        let records = snapshot(&reader, package_id, flag, limits, started)?;
        let mut prepared = PreparedDuplicateCheck {
            reader,
            versions: Vec::with_capacity(records.len()),
            records,
            package_id,
        };
        // Freeze each namespace version (or unavailable reason) while preparing. Do not adopt a replacement at Start.
        for record in &prepared.records {
            guard(flag, started, limits.work)?;
            prepared.versions.push(
                open_record(&prepared, record, None)
                    .map(|source| source.version)
                    .map_err(|reason| reason.chars().take(300).collect()),
            );
        }
        prepared.reader.validate()?;
        guard(flag, started, limits.work)?;
        Ok(prepared)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, package_id, flag);
        Err("Datei-Duplikatprüfungen sind derzeit nur für macOS qualifiziert".into())
    }
}
#[cfg(target_os = "macos")]
pub(super) mod sdk {
    #[repr(C)]
    #[derive(Default)]
    struct Context {
        count: [u32; 2],
        hash: [u32; 8],
        wbuf: [u32; 16],
    }
    const _: () =
        assert!(std::mem::size_of::<Context>() == 104 && std::mem::align_of::<Context>() == 4);
    extern "C" {
        fn CC_SHA256_Init(c: *mut Context) -> i32;
        fn CC_SHA256_Update(c: *mut Context, data: *const std::ffi::c_void, len: u32) -> i32;
        fn CC_SHA256_Final(out: *mut u8, c: *mut Context) -> i32;
    }
    pub(in crate::library) struct Sha256(Context);
    impl Sha256 {
        pub(in crate::library) fn new() -> Self {
            let mut c = Context::default();
            // SAFETY: public macOS CommonDigest.h SHA256 context ABI; Init writes this aligned owned context.
            unsafe {
                CC_SHA256_Init(&mut c);
            }
            Self(c)
        }
        pub(in crate::library) fn update(&mut self, data: &[u8]) {
            assert!(data.len() <= 65536);
            // SAFETY: live initialized context and readable slice; length is bounded to CC_LONG.
            unsafe {
                CC_SHA256_Update(&mut self.0, data.as_ptr().cast(), data.len() as u32);
            }
        }
        pub(in crate::library) fn finish(mut self) -> [u8; 32] {
            let mut out = [0; 32];
            // SAFETY: initialized context and writable SHA256_DIGEST_LENGTH (32-byte) output.
            unsafe {
                CC_SHA256_Final(out.as_mut_ptr(), &mut self.0);
            }
            out
        }
    }
}
#[cfg(target_os = "macos")]
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Version {
    pub(super) parents: Vec<super::storage::Stamp>,
    pub(super) stamp: super::storage::Stamp,
    pub(super) physical: String,
}
#[cfg(target_os = "macos")]
pub(super) struct Source {
    pub(super) file: fs::File,
    pub(super) version: Version,
}
#[cfg(target_os = "macos")]
pub(super) fn open_record(
    prepared: &PreparedDuplicateCheck,
    record: &Record,
    expected: Option<&Version>,
) -> Result<Source> {
    use super::storage::{anchored, Stamp};
    use std::os::unix::fs::MetadataExt;
    let (mut directory, relative) = if let Some(external) = &record.external {
        let root = anchored::root(&external.root)?;
        let metadata = root.metadata().map_err(|e| e.to_string())?;
        let identity = format!(
            "mac-volume:{}:{}",
            super::volume::uuid_from_file(&root)?,
            metadata.ino()
        );
        if identity != external.root_identity {
            return Err("Externe Quelle wurde ersetzt oder hat eine ungeprüfte Identität".into());
        }
        (
            root,
            normal_path(&record.file.relative_path)?
                .strip_prefix(&record.package_path)
                .map_err(|_| "Ungültiger externer Pfad")?
                .to_owned(),
        )
    } else {
        (
            prepared
                .reader
                .directory
                .try_clone()
                .map_err(|e| e.to_string())?,
            normal_path(&record.file.relative_path)?,
        )
    };
    let parts = relative.components().collect::<Vec<_>>();
    if parts.is_empty() {
        return Err("Variantenpfad ist leer".into());
    }
    let mut parents = Vec::new();
    let mut file = None;
    for (index, part) in parts.iter().enumerate() {
        let metadata = directory.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_dir() {
            return Err("Variantenordner wurde ersetzt".into());
        }
        parents.push(Stamp::read(&metadata));
        let next = anchored::child(&directory, part.as_os_str())?;
        if index + 1 == parts.len() {
            file = Some(next);
        } else {
            directory = next;
        }
    }
    let file = file.unwrap();
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() != record.file.size_bytes {
        return Err("Variantendatei wurde geändert oder ist nicht regulär".into());
    }
    let physical = format!(
        "mac-volume:{}:{}",
        super::volume::uuid_from_file(&file)?,
        metadata.ino()
    );
    if let Some(external) = &record.external {
        if physical != external.identity
            || format!("{:?}", metadata.modified().map_err(|e| e.to_string())?) != external.modified
        {
            return Err("Externe Variantendatei wurde geändert oder ersetzt".into());
        }
    }
    let version = Version {
        parents,
        stamp: Stamp::read(&metadata),
        physical,
    };
    if expected.is_some_and(|v| *v != version) {
        return Err("Variantendatei oder ihr übergeordneter Ordner wurde verändert".into());
    }
    Ok(Source { file, version })
}
// Final publication checks only need pinned metadata. Opening/closing a regular
// file here would release the process's SQLite lock if that file aliases the catalog.
#[cfg(target_os = "macos")]
pub(super) fn record_stamps(
    prepared: &PreparedDuplicateCheck,
    record: &Record,
) -> Result<(Vec<super::storage::Stamp>, super::storage::Stamp)> {
    use super::storage::{anchored, Stamp};
    use std::os::unix::fs::MetadataExt;
    let (mut directory, relative) = if let Some(external) = &record.external {
        let root = anchored::root(&external.root)?;
        let metadata = root.metadata().map_err(|e| e.to_string())?;
        if format!(
            "mac-volume:{}:{}",
            super::volume::uuid_from_file(&root)?,
            metadata.ino()
        ) != external.root_identity
        {
            return Err("Externe Quelle wurde ersetzt oder hat eine ungeprüfte Identität".into());
        }
        (
            root,
            normal_path(&record.file.relative_path)?
                .strip_prefix(&record.package_path)
                .map_err(|_| "Ungültiger externer Pfad")?
                .to_owned(),
        )
    } else {
        (
            prepared
                .reader
                .directory
                .try_clone()
                .map_err(|e| e.to_string())?,
            normal_path(&record.file.relative_path)?,
        )
    };
    let parts = relative.components().collect::<Vec<_>>();
    let mut parents = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let metadata = directory.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_dir() {
            return Err("Variantenordner wurde ersetzt".into());
        }
        parents.push(Stamp::read(&metadata));
        if index + 1 == parts.len() {
            let stamp = anchored::child_stamp(&directory, part.as_os_str())?;
            if !stamp.is_file() || stamp.size != record.file.size_bytes {
                return Err("Variantendatei wurde geändert oder ist nicht regulär".into());
            }
            return Ok((parents, stamp));
        }
        directory = anchored::directory(&directory, part.as_os_str())?;
    }
    Err("Variantenpfad ist leer".into())
}
#[cfg(target_os = "macos")]
pub(super) fn verify_source(
    prepared: &PreparedDuplicateCheck,
    record: &Record,
    source: &Source,
) -> Result<()> {
    if super::storage::Stamp::read(&source.file.metadata().map_err(|e| e.to_string())?)
        != source.version.stamp
    {
        return Err("Variantendatei wurde während des Lesens verändert".into());
    }
    open_record(prepared, record, Some(&source.version)).map(|_| ())
}
struct Budget<'a, F> {
    flag: &'a AtomicBool,
    started: Instant,
    limits: Limits,
    bytes: u64,
    exhausted: bool,
    processed: usize,
    total: usize,
    stage: &'static str,
    last: Instant,
    progress: F,
}
impl<F: FnMut(DuplicateProgress)> Budget<'_, F> {
    fn check(&self) -> Result<()> {
        guard(self.flag, self.started, self.limits.work)?;
        if self.exhausted {
            return Err("Lesebudget der Duplikatprüfung erreicht".into());
        }
        Ok(())
    }
    fn emit(&mut self, stage: &'static str, force: bool) {
        if force || stage != self.stage || self.last.elapsed() >= Duration::from_millis(100) {
            self.stage = stage;
            self.last = Instant::now();
            (self.progress)(DuplicateProgress {
                stage,
                total: self.total,
                processed: self.processed,
                bytes_read: self.bytes,
            });
        }
    }
    fn read(&mut self, file: &mut impl Read, buffer: &mut [u8]) -> Result<usize> {
        loop {
            self.check()?;
            let left = self.limits.bytes.saturating_sub(self.bytes);
            if left == 0 && !buffer.is_empty() {
                self.exhausted = true;
                return Err("Lesebudget der Duplikatprüfung erreicht".into());
            }
            let length = buffer.len().min(left.min(65536) as usize);
            match file.read(&mut buffer[..length]) {
                Ok(count) => {
                    let first = self.bytes == 0;
                    self.bytes = self
                        .bytes
                        .checked_add(count as u64)
                        .filter(|b| *b <= self.limits.bytes)
                        .ok_or("Lesebudget der Duplikatprüfung erreicht")?;
                    self.emit(self.stage, first && count > 0);
                    return Ok(count);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}
#[cfg(target_os = "macos")]
fn fingerprint<F: FnMut(DuplicateProgress)>(
    prepared: &PreparedDuplicateCheck,
    record: &Record,
    pinned: &Result<Version>,
    budget: &mut Budget<F>,
) -> Result<([u8; 32], Version)> {
    if record.file.size_bytes > budget.limits.file {
        return Err("Variante überschreitet das 4-GiB-Limit".into());
    }
    let mut source = open_record(
        prepared,
        record,
        Some(pinned.as_ref().map_err(Clone::clone)?),
    )?;
    let mut sha = sdk::Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut read = 0u64;
    while read < record.file.size_bytes {
        let length = buffer.len().min((record.file.size_bytes - read) as usize);
        let count = budget.read(&mut source.file, &mut buffer[..length])?;
        if count == 0 {
            break;
        }
        read = read
            .checked_add(count as u64)
            .filter(|v| *v <= record.file.size_bytes)
            .ok_or("Variante wurde beim Lesen größer")?;
        sha.update(&buffer[..count]);
    }
    if read != record.file.size_bytes {
        return Err("Variante wurde beim Lesen gekürzt".into());
    }
    budget.check()?;
    verify_source(prepared, record, &source)?;
    Ok((sha.finish(), source.version))
}
#[cfg(target_os = "macos")]
fn equal<F: FnMut(DuplicateProgress)>(
    prepared: &PreparedDuplicateCheck,
    left: &Record,
    lv: &Version,
    right: &Record,
    rv: &Version,
    budget: &mut Budget<F>,
) -> Result<bool> {
    let mut left_source = open_record(prepared, left, Some(lv))?;
    let mut right_source = open_record(prepared, right, Some(rv))?;
    let mut a = [0; 65536];
    let mut b = [0; 65536];
    let mut remaining = left.file.size_bytes;
    let mut same = true;
    while remaining > 0 {
        let count = (remaining as usize).min(a.len());
        let mut fill = |file: &mut fs::File, buf: &mut [u8]| -> Result<()> {
            let mut offset = 0;
            while offset < buf.len() {
                let n = budget.read(file, &mut buf[offset..])?;
                if n == 0 {
                    return Err("Variante wurde während des Vergleichs gekürzt".into());
                }
                offset += n;
            }
            Ok(())
        };
        fill(&mut left_source.file, &mut a[..count])?;
        fill(&mut right_source.file, &mut b[..count])?;
        if a[..count] != b[..count] {
            same = false;
            break;
        }
        remaining -= count as u64;
    }
    budget.check()?;
    verify_source(prepared, left, &left_source)?;
    verify_source(prepared, right, &right_source)?;
    Ok(same)
}
fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
pub(crate) fn run_duplicate_check(
    prepared: PreparedDuplicateCheck,
    flag: &AtomicBool,
    progress: impl FnMut(DuplicateProgress),
) -> Result<DuplicateReport> {
    run_with_limits(prepared, flag, progress, LIMITS)
}
fn run_with_limits(
    prepared: PreparedDuplicateCheck,
    flag: &AtomicBool,
    progress: impl FnMut(DuplicateProgress),
    limits: Limits,
) -> Result<DuplicateReport> {
    run_with_candidate_key(prepared, flag, progress, limits, |digest| digest)
}
// Candidate keys may nominate a comparison, but never establish equality.
fn run_with_candidate_key(
    prepared: PreparedDuplicateCheck,
    flag: &AtomicBool,
    progress: impl FnMut(DuplicateProgress),
    limits: Limits,
    mut candidate_key: impl FnMut([u8; 32]) -> [u8; 32],
) -> Result<DuplicateReport> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (prepared, flag, progress, limits, candidate_key);
        Err("Datei-Duplikatprüfungen sind derzeit nur für macOS qualifiziert".into())
    }
    #[cfg(target_os = "macos")]
    {
        let started = Instant::now();
        prepared.reader.validate()?;
        let mut report = DuplicateReport {
            complete: true,
            cancelled: false,
            started_at_ms: now_ms(),
            finished_at_ms: 0,
            total: prepared.total(),
            processed: 0,
            bytes_read: 0,
            groups: Vec::new(),
            files: prepared.records.iter().map(|r| r.file.clone()).collect(),
            issues: Vec::new(),
        };
        let mut budget = Budget {
            flag,
            started,
            limits,
            bytes: 0,
            exhausted: false,
            processed: 0,
            total: report.total,
            stage: "hash",
            last: Instant::now(),
            progress,
        };
        budget.emit("hash", true);
        let mut measured = Vec::with_capacity(report.total);
        let mut buckets = BTreeMap::<(u64, [u8; 32]), Vec<usize>>::new();
        for (index, record) in prepared.records.iter().enumerate() {
            if budget.check().is_err() {
                break;
            }
            match fingerprint(&prepared, record, &prepared.versions[index], &mut budget) {
                Ok((digest, version)) => {
                    report.files[index].status = "checked";
                    report.files[index].sha256 = Some(hex(&digest));
                    buckets
                        .entry((record.file.size_bytes, candidate_key(digest)))
                        .or_default()
                        .push(index);
                    measured.push(Some(version));
                }
                Err(reason) => {
                    report.complete = false;
                    report.files[index].status = "unavailable";
                    report.files[index].reason = Some(reason.chars().take(300).collect());
                    measured.push(None);
                }
            }
            budget.processed += 1;
            budget.emit("hash", false);
        }
        budget.emit("compare", true);
        let mut groups = Vec::new();
        'buckets: for ((size, _), members) in buckets {
            let mut classes: Vec<Vec<usize>> = Vec::new();
            for index in members {
                if budget.check().is_err() {
                    break 'buckets;
                }
                let mut found = false;
                for class in &mut classes {
                    let other = class[0];
                    match equal(
                        &prepared,
                        &prepared.records[other],
                        measured[other].as_ref().unwrap(),
                        &prepared.records[index],
                        measured[index].as_ref().unwrap(),
                        &mut budget,
                    ) {
                        Ok(true) => {
                            class.push(index);
                            found = true;
                            break;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            report.complete = false;
                            for i in [other, index] {
                                report.files[i].status = "unavailable";
                                report.files[i].sha256 = None;
                                report.files[i].reason = Some(error.chars().take(300).collect());
                            }
                            report.issue(error);
                            break 'buckets;
                        }
                    }
                }
                if !found {
                    classes.push(vec![index]);
                }
            }
            for class in classes.into_iter().filter(|c| c.len() > 1) {
                let identities = class
                    .iter()
                    .map(|i| &measured[*i].as_ref().unwrap().physical)
                    .collect::<std::collections::HashSet<_>>();
                groups.push(DuplicateGroup {
                    sha256: report.files[class[0]].sha256.clone().unwrap(),
                    size_bytes: size,
                    members: class
                        .iter()
                        .map(|i| prepared.records[*i].file.file_id)
                        .collect(),
                    shared_source: identities.len() < class.len(),
                });
            }
        }
        budget.emit("verify", true);
        let mut valid = true;
        for (index, version) in measured.iter().enumerate() {
            if budget.check().is_err() {
                valid = false;
                break;
            }
            if let Some(version) = version {
                if let Err(error) = open_record(&prepared, &prepared.records[index], Some(version))
                {
                    valid = false;
                    report.files[index].status = "unavailable";
                    report.files[index].sha256 = None;
                    report.files[index].reason = Some(error.chars().take(300).collect());
                }
            }
        }
        if budget.check().is_err() {
            valid = false;
            report.complete = false;
            report.cancelled = flag.load(Ordering::Relaxed);
            report.issue(if report.cancelled {
                "Duplikatprüfung abgebrochen"
            } else {
                "Zeit- oder Lesebudget erreicht; Ergebnis unvollständig"
            });
        }
        if let Err(error) = prepared.reader.validate() {
            valid = false;
            report.complete = false;
            report.issue(error);
        }
        if valid {
            match snapshot(
                &prepared.reader,
                prepared.package_id,
                flag,
                limits,
                budget.started,
            ) {
                Ok(records) if records == prepared.records => {
                    if let Err(error) = prepared.reader.validate() {
                        valid = false;
                        report.issue(error);
                    }
                }
                Ok(_) => valid = false,
                Err(error) => {
                    valid = false;
                    report.issue(error);
                }
            }
        }
        // Final SQL and namespace validation share the original run deadline; none renew the lease.
        if budget.check().is_err() {
            valid = false;
            report.cancelled = flag.load(Ordering::Relaxed);
            report.issue(if report.cancelled {
                "Duplikatprüfung abgebrochen"
            } else {
                "Zeit- oder Lesebudget erreicht; Ergebnis unvollständig"
            });
        }
        if valid {
            report.groups = groups;
        } else {
            report.complete = false;
            report
                .issue("Quellen oder Katalogzuordnung konnten nicht abschließend bestätigt werden");
        }
        report.processed = budget.processed;
        report.complete &= report.processed == report.total;
        report.bytes_read = budget.bytes;
        report.finished_at_ms = now_ms();
        budget.emit("finished", true);
        Ok(report)
    }
}
#[cfg(all(test, target_os = "macos"))]
#[path = "duplicate_tests.rs"]
mod tests;
