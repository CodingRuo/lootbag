use super::{AssetPackage, ImportControl, ImportProgress, Result, ZipEstimate};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparationProgress {
    pub stage: &'static str,
    pub examined_candidates: usize,
    pub total_candidates: Option<usize>,
    pub bytes_read: u64,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IncomingMatch {
    pub incoming_relative_path: String,
    pub size_bytes: u64,
    pub existing_file_id: i64,
    pub existing_asset_id: i64,
    pub existing_package_id: i64,
    pub existing_package_name: String,
    pub existing_original_filename: String,
    pub existing_relative_path: String,
    pub sha256: String,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Evidence {
    pub status: &'static str,
    pub incoming_files: Option<usize>,
    pub candidates: Option<usize>,
    pub examined_candidates: usize,
    pub duplicate_incoming_files: usize,
    pub unavailable_candidates: usize,
    pub existing_variants: Option<usize>,
    pub unavailable_existing_variants: usize,
    pub matches: Vec<IncomingMatch>,
    pub issues: Vec<String>,
    pub details_truncated: bool,
}
impl Evidence {
    fn new() -> Self {
        Self {
            status: "incomplete",
            incoming_files: None,
            candidates: None,
            examined_candidates: 0,
            duplicate_incoming_files: 0,
            unavailable_candidates: 0,
            existing_variants: None,
            unavailable_existing_variants: 0,
            matches: vec![],
            issues: vec![],
            details_truncated: false,
        }
    }
    fn issue(&mut self, s: impl AsRef<str>) {
        if self.issues.len() < 30 {
            self.issues.push(s.as_ref().chars().take(300).collect())
        } else {
            self.details_truncated = true;
        }
    }
}
#[cfg(target_os = "macos")]
use super::{
    duplicates::{self, PreparedDuplicateCheck},
    storage::{anchored, Stamp},
};
#[cfg(target_os = "macos")]
#[derive(Clone)]
struct SourcePin {
    selected: PathBuf,
    path: PathBuf,
    stamp: Stamp,
    zip: bool,
    ancestors: Vec<(PathBuf, (u64, u64))>,
}
#[cfg(target_os = "macos")]
impl SourcePin {
    fn capture(root: &Path, source: &Path, zip: bool) -> Result<Self> {
        let metadata = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink()
            || (zip && !metadata.is_file())
            || (!zip && !metadata.is_dir())
        {
            return Err("Importquelle wurde ersetzt oder ist nicht zulässig".into());
        }
        let path = source.canonicalize().map_err(|e| e.to_string())?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        if path.starts_with(&root) || root.starts_with(&path) {
            return Err("Quellordner und Bibliothek dürfen nicht ineinander liegen".into());
        }
        let file = anchored::root_file(&path)?;
        let stamp = Stamp::read(&file.metadata().map_err(|e| e.to_string())?);
        let ancestors = path
            .ancestors()
            .skip(1)
            .map(|p| {
                Ok((
                    p.to_owned(),
                    Stamp::read(&anchored::root(p)?.metadata().map_err(|e| e.to_string())?)
                        .identity(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let pin = Self {
            selected: source.into(),
            path,
            stamp,
            zip,
            ancestors,
        };
        pin.validate()?;
        Ok(pin)
    }
    fn validate(&self) -> Result<fs::File> {
        for (path, id) in &self.ancestors {
            if Stamp::read(
                &anchored::root(path)?
                    .metadata()
                    .map_err(|e| e.to_string())?,
            )
            .identity()
                != *id
            {
                return Err(
                    "Übergeordneter Quellordner wurde ersetzt; bitte neu vorbereiten".into(),
                );
            }
        }
        let m = fs::symlink_metadata(&self.selected).map_err(|e| e.to_string())?;
        if m.file_type().is_symlink()
            || self.selected.canonicalize().ok().as_deref() != Some(self.path.as_path())
        {
            return Err(
                "Importquelle oder übergeordneter Pfad wurde ersetzt; bitte neu vorbereiten".into(),
            );
        }
        let file = if self.zip {
            anchored::root_file(&self.path)?
        } else {
            anchored::root(&self.path)?
        };
        let m = file.metadata().map_err(|e| e.to_string())?;
        if Stamp::read(&m) != self.stamp || (self.zip && !m.is_file()) || (!self.zip && !m.is_dir())
        {
            return Err("Importquelle wurde verändert; bitte neu vorbereiten".into());
        }
        Ok(file)
    }
}
#[cfg(target_os = "macos")]
struct DestinationPin {
    path: PathBuf,
    directory: fs::File,
    identity: (u64, u64),
    protected: Vec<(u64, u64)>,
}
#[cfg(target_os = "macos")]
fn protected_catalog_sources(directory: &fs::File) -> Result<Vec<(u64, u64)>> {
    let mut identities = vec![];
    for name in [
        "catalog.sqlite3",
        "catalog.sqlite3-journal",
        "catalog.sqlite3-wal",
        "catalog.sqlite3-shm",
    ] {
        if let Some(stamp) = anchored::optional_child_stamp(directory, std::ffi::OsStr::new(name))?
        {
            identities.push(stamp.identity());
        }
    }
    Ok(identities)
}
#[cfg(target_os = "macos")]
impl DestinationPin {
    fn capture(root: &Path) -> Result<Self> {
        let path = root.canonicalize().map_err(|e| e.to_string())?;
        let directory = anchored::root(&path)?;
        let identity = Stamp::read(&directory.metadata().map_err(|e| e.to_string())?).identity();
        let protected = protected_catalog_sources(&directory)?;
        Ok(Self {
            path,
            directory,
            identity,
            protected,
        })
    }
    fn validate(&self, root: &Path) -> Result<()> {
        if root.canonicalize().ok().as_deref() != Some(self.path.as_path())
            || Stamp::read(&self.directory.metadata().map_err(|e| e.to_string())?).identity()
                != self.identity
            || Stamp::read(
                &anchored::root(&self.path)?
                    .metadata()
                    .map_err(|e| e.to_string())?,
            )
            .identity()
                != self.identity
        {
            return Err("Bibliotheksordner wurde ersetzt; bitte neu vorbereiten".into());
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    stamp: Stamp,
    directory: bool,
    size: u64,
    candidate: bool,
    digest: Option<[u8; 32]>,
}
#[cfg(target_os = "macos")]
struct FolderPlan {
    entries: Vec<Entry>,
    existing: PreparedDuplicateCheck,
}
pub(crate) struct PreparedPackage {
    pub(crate) source: PathBuf,
    pub(crate) zip: bool,
    pub(crate) evidence: Evidence,
    #[cfg(target_os = "macos")]
    pin: SourcePin,
    #[cfg(target_os = "macos")]
    destination: DestinationPin,
    #[cfg(target_os = "macos")]
    folder: Option<FolderPlan>,
}
impl PreparedPackage {
    pub(crate) fn verified(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.folder.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
    pub(crate) fn validate_selection(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.pin.validate().map(|_| ())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Vorbereiteter Import ist derzeit nur für macOS qualifiziert".into())
        }
    }
}
#[derive(Clone, Copy)]
struct Limits {
    entries: usize,
    retained: usize,
    file: u64,
    total: u64,
    reads: u64,
    work: Duration,
}
const LIMITS: Limits = Limits {
    entries: 20_000,
    retained: 32 * 1024 * 1024,
    file: 256 * 1024 * 1024,
    total: 1024 * 1024 * 1024,
    reads: 2 * 1024 * 1024 * 1024,
    work: Duration::from_secs(30),
};
struct Work<'a, F> {
    control: &'a ImportControl,
    started: Instant,
    limits: Limits,
    charged: u64,
    bytes: u64,
    examined: usize,
    total: Option<usize>,
    progress: F,
    stage: &'static str,
    last_progress: Instant,
}
impl<F: FnMut(PreparationProgress)> Work<'_, F> {
    fn check_cancelled(&self) -> Result<()> {
        if self
            .control
            .cancellation_flag()
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err("Importvorbereitung abgebrochen".into());
        }
        Ok(())
    }
    fn check(&self) -> Result<()> {
        self.check_cancelled()?;
        if self.started.elapsed() >= self.limits.work {
            return Err("Zeitlimit der Importprüfung erreicht".into());
        }
        Ok(())
    }
    fn emit(&mut self, stage: &'static str) {
        if self.stage == stage && self.last_progress.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.stage = stage;
        self.last_progress = Instant::now();
        (self.progress)(PreparationProgress {
            stage,
            examined_candidates: self.examined,
            total_candidates: self.total,
            bytes_read: self.bytes,
        })
    }
    fn read(&mut self, file: &mut impl std::io::Read, buf: &mut [u8]) -> Result<usize> {
        self.check()?;
        let n = buf.len().min(65536);
        self.charged = self
            .charged
            .checked_add(n as u64)
            .filter(|v| *v <= self.limits.reads)
            .ok_or("Lesebudget der Importprüfung erreicht")?;
        let n = file.read(&mut buf[..n]).map_err(|e| e.to_string())?;
        self.bytes += n as u64;
        self.emit(self.stage);
        Ok(n)
    }
}
#[cfg(target_os = "macos")]
fn inventory<F: FnMut(PreparationProgress)>(
    root: &fs::File,
    work: &mut Work<F>,
) -> Result<Vec<Entry>> {
    fn walk<F: FnMut(PreparationProgress)>(
        root: &fs::File,
        dir: &fs::File,
        relative: &Path,
        entries: &mut Vec<Entry>,
        retained: &mut usize,
        total: &mut u64,
        work: &mut Work<F>,
    ) -> Result<()> {
        work.check()?;
        if relative.components().count() > 48 {
            return Err("Ordnerprüfung überschreitet das Tiefenlimit".into());
        }
        let listing = anchored::Listing::new(dir)?;
        while let Some(name) = listing.next()? {
            work.check()?;
            if entries.len() >= work.limits.entries {
                return Err("Ordnerprüfung überschreitet das Eintragslimit".into());
            }
            let path = relative.join(&name);
            let text = path
                .to_str()
                .ok_or("Ordnerprüfung benötigt UTF-8-Dateinamen")?;
            *retained = retained
                .checked_add(text.len() + 256)
                .ok_or("Ordnerprüfung ist zu groß")?;
            if text.len() > 4096 || *retained > work.limits.retained {
                return Err("Ordnerprüfung überschreitet das Pfadlimit".into());
            }
            let stamp = anchored::child_stamp(dir, &name)?;
            if !stamp.is_dir() && !stamp.is_file() {
                return Err("Ordnerprüfung enthält eine nicht reguläre Datei".into());
            }
            if stamp.is_file() {
                *total = total
                    .checked_add(stamp.size)
                    .ok_or("Ordnerprüfung ist zu groß")?;
                if stamp.size > work.limits.file || *total > work.limits.total {
                    return Err("Ordnerprüfung überschreitet das Dateigrößenlimit; vollständiger ungeprüfter Import bleibt möglich".into());
                }
            }
            entries.push(Entry {
                path: path.clone(),
                stamp,
                directory: stamp.is_dir(),
                size: if stamp.is_file() { stamp.size } else { 0 },
                candidate: stamp.is_file() && super::assets::candidate_format(&path).is_some(),
                digest: None,
            });
            if stamp.is_dir() {
                let file = anchored::directory(dir, &name)?;
                if Stamp::read(&file.metadata().map_err(|e| e.to_string())?) != stamp {
                    return Err("Quellordner wurde während der Prüfung ersetzt".into());
                }
                walk(root, &file, &path, entries, retained, total, work)?
            }
        }
        Ok(())
    }
    let mut entries = vec![];
    walk(
        root,
        root,
        Path::new(""),
        &mut entries,
        &mut 0,
        &mut 0,
        work,
    )?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}
#[cfg(target_os = "macos")]
fn hash<F: FnMut(PreparationProgress)>(
    file: &mut fs::File,
    size: u64,
    work: &mut Work<F>,
) -> Result<[u8; 32]> {
    let mut sha = duplicates::sdk::Sha256::new();
    let mut buf = [0u8; 65536];
    let mut left = size;
    while left > 0 {
        let n = work.read(file, &mut buf[..left.min(65536) as usize])?;
        if n == 0 {
            return Err("Datei wurde gekürzt".into());
        }
        sha.update(&buf[..n]);
        left -= n as u64;
    }
    work.check()?;
    Ok(sha.finish())
}
#[cfg(target_os = "macos")]
fn identical<F: FnMut(PreparationProgress)>(
    a: &mut fs::File,
    b: &mut fs::File,
    size: u64,
    work: &mut Work<F>,
) -> Result<bool> {
    let mut left = size;
    let mut aa = [0; 65536];
    let mut bb = [0; 65536];
    while left > 0 {
        let n = left.min(65536) as usize;
        for (f, buf) in [(&mut *a, &mut aa[..n]), (&mut *b, &mut bb[..n])] {
            let mut offset = 0;
            while offset < n {
                let count = work.read(f, &mut buf[offset..])?;
                if count == 0 {
                    return Err("Datei wurde während des Vergleichs gekürzt".into());
                }
                offset += count;
            }
        }
        if aa[..n] != bb[..n] {
            return Ok(false);
        }
        left -= n as u64;
    }
    work.check()?;
    Ok(true)
}
pub(crate) fn prepare(
    root: &Path,
    source: &Path,
    zip: bool,
    control: &ImportControl,
    progress: impl FnMut(PreparationProgress),
) -> Result<PreparedPackage> {
    prepare_with_limits(root, source, zip, control, progress, LIMITS)
}
fn prepare_with_limits(
    root: &Path,
    source: &Path,
    zip: bool,
    control: &ImportControl,
    progress: impl FnMut(PreparationProgress),
    limits: Limits,
) -> Result<PreparedPackage> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, source, zip, control, progress, limits);
        Err("Vorbereiteter Import ist derzeit nur für macOS qualifiziert".into())
    }
    #[cfg(target_os = "macos")]
    {
        let mut work = Work {
            control,
            started: Instant::now(),
            limits,
            charged: 0,
            bytes: 0,
            examined: 0,
            total: None,
            progress,
            stage: "",
            last_progress: Instant::now(),
        };
        work.check_cancelled()?;
        let pin = SourcePin::capture(root, source, zip)?;
        let mut plan = PreparedPackage {
            source: pin.path.clone(),
            zip,
            evidence: Evidence::new(),
            pin,
            destination: DestinationPin::capture(root)?,
            folder: None,
        };
        if zip
            && plan
                .destination
                .protected
                .contains(&plan.pin.stamp.identity())
        {
            return Err("ZIP-Quelle ist mit dem Bibliothekskatalog oder einer SQLite-Begleitdatei verknüpft".into());
        }
        if zip {
            plan.evidence.status = "unavailable";
            plan.evidence.issue("ZIP-Inhalte wurden nicht geprüft. Nach Bestätigung wird das gesamte Paket mit dem bisherigen ZIP-Importer übernommen.");
            return Ok(plan);
        }
        work.emit("inventory");
        let directory = plan.pin.validate()?;
        let mut entries = match inventory(&directory, &mut work) {
            Ok(v) => v,
            Err(e) => {
                work.check_cancelled()?;
                plan.evidence.issue(e);
                return Ok(plan);
            }
        };
        if entries.iter().any(|entry| {
            !entry.directory && plan.destination.protected.contains(&entry.stamp.identity())
        }) {
            return Err("Eine Importdatei ist mit dem Bibliothekskatalog oder einer SQLite-Begleitdatei verknüpft; bitte eine unabhängige Quelle verwenden".into());
        }
        let candidates = entries.iter().filter(|e| e.candidate).count();
        plan.evidence.incoming_files = Some(entries.iter().filter(|e| !e.directory).count());
        plan.evidence.candidates = Some(candidates);
        work.total = Some(candidates);
        let inspection = (|| -> Result<FolderPlan> {
            for entry in entries.iter_mut().filter(|e| !e.directory) {
                let mut file = anchored::relative(&directory, &entry.path)?;
                if Stamp::read(&file.metadata().map_err(|e| e.to_string())?) != entry.stamp {
                    return Err("Eingehende Datei wurde verändert".into());
                }
                entry.digest = Some(hash(&mut file, entry.size, &mut work)?);
                if Stamp::read(&file.metadata().map_err(|e| e.to_string())?) != entry.stamp {
                    return Err("Eingehende Datei wurde beim Lesen verändert".into());
                }
            }
            let existing = duplicates::prepare_preimport_catalog(
                root,
                control.cancellation_flag(),
                work.started,
                limits.work,
            )?;
            plan.evidence.existing_variants = Some(existing.total());
            // Bound retained Rust-owned plan structures. SQLite's separately bounded connection is not an RSS cap.
            let incoming_bytes = entries.capacity() * std::mem::size_of::<Entry>()
                + entries.iter().map(|e| e.path.capacity()).sum::<usize>();
            let pin_bytes = plan.pin.selected.capacity()
                + plan.pin.path.capacity()
                + plan.source.capacity()
                + plan.pin.ancestors.capacity() * std::mem::size_of::<(PathBuf, (u64, u64))>()
                + plan
                    .pin
                    .ancestors
                    .iter()
                    .map(|(p, _)| p.capacity())
                    .sum::<usize>();
            if incoming_bytes + pin_bytes + existing.retained_preimport_bytes()
                > limits.retained.saturating_sub(1024 * 1024)
            {
                return Err("Gespeicherter Prüfplan überschreitet das 32-MiB-Limit; vollständiger ungeprüfter Import bleibt möglich".into());
            }

            let mut hashes = vec![];
            for (index, record) in existing.records.iter().enumerate() {
                work.check()?;
                work.emit("compare");
                let result: Result<[u8; 32]> = (|| {
                    if record.file.size_bytes > limits.file {
                        return Err(
                            "Bestehende Variante überschreitet das Prüfungsgrößenlimit".into()
                        );
                    }
                    let version = existing.versions[index].as_ref().map_err(Clone::clone)?;
                    let mut file = duplicates::open_record(&existing, record, Some(version))?;
                    let digest = hash(&mut file.file, record.file.size_bytes, &mut work)?;
                    duplicates::verify_source(&existing, record, &file)?;
                    Ok(digest)
                })();
                match result {
                    Ok(d) => hashes.push((index, d)),
                    Err(e) => {
                        plan.evidence.unavailable_existing_variants += 1;
                        plan.evidence.issue(e);
                    }
                }
            }
            for entry in entries.iter().filter(|e| e.candidate) {
                work.check()?;
                let mut duplicate = false;
                for (index, digest) in &hashes {
                    let record = &existing.records[*index];
                    if entry.size != record.file.size_bytes || entry.digest != Some(*digest) {
                        continue;
                    }
                    let mut incoming = anchored::relative(&directory, &entry.path)?;
                    let mut old = duplicates::open_record(
                        &existing,
                        record,
                        Some(existing.versions[*index].as_ref().map_err(Clone::clone)?),
                    )?;
                    if identical(&mut incoming, &mut old.file, entry.size, &mut work)? {
                        duplicates::verify_source(&existing, record, &old)?;
                        duplicate = true;
                        if plan.evidence.matches.len() < 30 {
                            plan.evidence.matches.push(IncomingMatch {
                                incoming_relative_path: entry.path.to_str().unwrap().into(),
                                size_bytes: entry.size,
                                existing_file_id: record.file.file_id,
                                existing_asset_id: record.file.asset_id,
                                existing_package_id: record.file.package_id,
                                existing_package_name: record.file.package_name.clone(),
                                existing_original_filename: record.file.original_filename.clone(),
                                existing_relative_path: record.file.relative_path.clone(),
                                sha256: digest.iter().map(|b| format!("{b:02x}")).collect(),
                            });
                        } else {
                            plan.evidence.details_truncated = true;
                        }
                    }
                }
                if duplicate {
                    plan.evidence.duplicate_incoming_files += 1
                }
                work.examined += 1;
                plan.evidence.examined_candidates = work.examined;
                work.emit("compare");
            }
            work.emit("verify");
            plan.pin.validate()?;
            existing.reader.validate()?;
            for (index, record) in existing.records.iter().enumerate() {
                if let Ok(version) = &existing.versions[index] {
                    duplicates::open_record(&existing, record, Some(version))?;
                }
            }
            let current = inventory(&directory, &mut work)?;
            if current
                .iter()
                .zip(&entries)
                .any(|(a, b)| a.path != b.path || a.stamp != b.stamp || a.directory != b.directory)
                || current.len() != entries.len()
            {
                return Err("Ordnerinhalt wurde während der Vorbereitung verändert".into());
            }
            work.check()?;
            Ok(FolderPlan { entries, existing })
        })();
        match inspection {
            Ok(folder) if plan.evidence.unavailable_existing_variants == 0 => {
                plan.evidence.status = "complete";
                plan.folder = Some(folder)
            }
            Ok(_) => {}
            Err(e) => {
                work.check_cancelled()?;
                plan.evidence.issue(e);
                plan.evidence.unavailable_candidates =
                    candidates.saturating_sub(plan.evidence.examined_candidates);
                plan.evidence.matches.clear();
                plan.evidence.duplicate_incoming_files = 0;
            }
        }
        Ok(plan)
    }
}
#[cfg(all(test, target_os = "macos"))]
#[path = "preimport_tests.rs"]
mod tests;
#[cfg(target_os = "macos")]
fn children<F: FnMut(PreparationProgress)>(
    directory: &fs::File,
    except: Option<&std::ffi::OsStr>,
    work: &Work<F>,
) -> Result<Vec<(std::ffi::OsString, Stamp)>> {
    let list = anchored::Listing::new(directory)?;
    let mut children = vec![];
    while let Some(name) = list.next()? {
        work.check()?;
        if except == Some(name.as_os_str()) {
            continue;
        }
        if children.len() >= 20_000 {
            return Err("Paketspeicher überschreitet das Übergangsprüfungslimit".into());
        }
        let stamp = anchored::child_stamp(directory, &name)?;
        children.push((name, stamp))
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(children)
}
#[cfg(target_os = "macos")]
struct Stage {
    root: fs::File,
    packages: fs::File,
    directory: fs::File,
    root_stamp: Stamp,
    packages_stamp: Stamp,
    root_children: Vec<(std::ffi::OsString, Stamp)>,
    siblings: Vec<(std::ffi::OsString, Stamp)>,
    name: String,
    key: String,
    identity: (u64, u64),
    outputs: Vec<(PathBuf, Stamp, bool)>,
    published: bool,
}
#[cfg(target_os = "macos")]
impl Stage {
    fn create<F: FnMut(PreparationProgress)>(plan: &FolderPlan, work: &Work<F>) -> Result<Self> {
        plan.existing.reader.validate()?;
        let root = plan
            .existing
            .reader
            .directory
            .try_clone()
            .map_err(|e| e.to_string())?;
        let root_children = children(&root, Some(std::ffi::OsStr::new("packages")), work)?;
        plan.existing.reader.validate()?;
        let (packages, created_packages) =
            match anchored::child(&root, std::ffi::OsStr::new("packages")) {
                Ok(f) => (f, false),
                Err(_) => (
                    super::sprite_export::anchored::create_at_report_retained(
                        &root,
                        "packages",
                        || work.control.mark_cleanup_failed(),
                    )?,
                    true,
                ),
            };
        if !packages.metadata().map_err(|e| e.to_string())?.is_dir() {
            return Err("Paketspeicher ist kein Ordner".into());
        }
        let siblings = children(&packages, None, work)?;
        // Recheck the original versions after observing namespaces; only the following stage transition is admitted.
        if !created_packages {
            plan.existing.reader.validate()?;
        }
        for (i, record) in plan.existing.records.iter().enumerate() {
            work.check()?;
            duplicates::open_record(
                &plan.existing,
                record,
                Some(plan.existing.versions[i].as_ref().map_err(Clone::clone)?),
            )?;
        }
        let key = super::unique_suffix();
        let name = format!(".import-{key}");
        let directory =
            super::sprite_export::anchored::create_at_report_retained(&packages, &name, || {
                work.control.mark_cleanup_failed()
            })?;
        let identity = Stamp::read(&directory.metadata().map_err(|e| e.to_string())?).identity();
        let stage = Self {
            root_stamp: Stamp::read(&root.metadata().map_err(|e| e.to_string())?),
            packages_stamp: Stamp::read(&packages.metadata().map_err(|e| e.to_string())?),
            root,
            packages,
            directory,
            root_children,
            siblings,
            name,
            key,
            identity,
            outputs: vec![],
            published: false,
        };
        if let Err(error) = stage.validate(&plan.existing, work) {
            if !stage.cleanup() {
                work.control.mark_cleanup_failed();
                return Err(format!(
                    "{error}; der veränderte Zwischenordner wurde nicht vollständig entfernt"
                ));
            }
            return Err(error);
        }
        Ok(stage)
    }
    fn check_containers(&self, existing: &PreparedDuplicateCheck) -> Result<()> {
        existing.reader.validate_with_root(self.root_stamp)?;
        if Stamp::read(&self.root.metadata().map_err(|e| e.to_string())?) != self.root_stamp
            || Stamp::read(&self.packages.metadata().map_err(|e| e.to_string())?)
                != self.packages_stamp
            || Stamp::read(
                &anchored::directory(&self.root, std::ffi::OsStr::new("packages"))?
                    .metadata()
                    .map_err(|e| e.to_string())?,
            ) != self.packages_stamp
            || Stamp::read(
                &anchored::directory(&self.packages, std::ffi::OsStr::new(&self.name))?
                    .metadata()
                    .map_err(|e| e.to_string())?,
            )
            .identity()
                != self.identity
        {
            return Err("Import-Staging oder Paketspeicher wurde verändert".into());
        }
        Ok(())
    }
    fn validate<F: FnMut(PreparationProgress)>(
        &self,
        existing: &PreparedDuplicateCheck,
        work: &Work<F>,
    ) -> Result<()> {
        work.check()?;
        self.check_containers(existing)?;
        if children(&self.root, Some(std::ffi::OsStr::new("packages")), work)? != self.root_children
            || children(&self.packages, Some(std::ffi::OsStr::new(&self.name)), work)?
                != self.siblings
        {
            return Err("Import-Staging oder Paketspeicher wurde verändert".into());
        }
        for (i, record) in existing.records.iter().enumerate() {
            work.check()?;
            let old = existing.versions[i].as_ref().map_err(Clone::clone)?;
            let (parents, stamp) = duplicates::record_stamps(existing, record)?;
            let mut expected = old.clone();
            if record.external.is_none() {
                if expected.parents.len() < 2 {
                    return Err("Variantenpfad ist unvollständig".into());
                }
                expected.parents[0] = self.root_stamp;
                expected.parents[1] = self.packages_stamp;
            }
            if parents != expected.parents || stamp != expected.stamp {
                return Err("Bestehende Variante wurde verändert; bitte neu vorbereiten".into());
            }
        }
        Ok(())
    }
    fn verify_outputs<F: FnMut(PreparationProgress)>(
        &self,
        entries: &[Entry],
        work: &mut Work<F>,
    ) -> Result<Vec<Entry>> {
        let current = inventory(&self.directory, work)?;
        if current.len() != entries.len() {
            return Err("Staging-Inhalt wurde verändert".into());
        }
        for (a, b) in current.iter().zip(entries) {
            if a.path != b.path || a.directory != b.directory || a.size != b.size {
                return Err("Staging-Topologie wurde verändert".into());
            }
            if !a.directory {
                let mut file = anchored::relative(&self.directory, &a.path)?;
                let digest = hash(&mut file, a.size, work)?;
                if Some(digest) != b.digest
                    || Stamp::read(&file.metadata().map_err(|e| e.to_string())?) != a.stamp
                {
                    return Err("Kopierte Datei wurde verändert".into());
                }
            }
        }
        Ok(current)
    }
    fn publish(&mut self) -> Result<()> {
        super::sprite_export::anchored::publish(
            &self.packages,
            &self.name,
            std::ffi::OsStr::new(&self.key),
        )?;
        self.published = true;
        Ok(())
    }
    fn cleanup(&self) -> bool {
        for (path, stamp, dir) in self.outputs.iter().rev() {
            let Ok(parent) = (if let Some(p) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                anchored::relative(&self.directory, p)
            } else {
                self.directory.try_clone().map_err(|e| e.to_string())
            }) else {
                continue;
            };
            if let Some(name) = path.file_name() {
                let _ = super::sprite_export::anchored::remove_owned(
                    &parent,
                    name,
                    stamp.identity(),
                    *dir,
                );
            }
        }
        let name = if self.published {
            &self.key
        } else {
            &self.name
        };
        super::sprite_export::anchored::remove_owned(
            &self.packages,
            std::ffi::OsStr::new(name),
            self.identity,
            true,
        )
    }
}
#[cfg(target_os = "macos")]
fn consume_folder(
    root: &Path,
    prepared: PreparedPackage,
    control: &ImportControl,
    progress: &mut dyn FnMut(ImportProgress),
    publisher: &mut super::imports::Publication<'_>,
) -> Result<AssetPackage> {
    let source = prepared.pin.validate()?;
    let mut folder = prepared.folder.ok_or("Ordnerplan ist nicht geprüft")?;
    if folder
        .existing
        .versions
        .iter()
        .filter_map(|version| version.as_ref().ok())
        .any(|version| {
            prepared
                .destination
                .protected
                .contains(&version.stamp.identity())
        })
    {
        return Err("Eine bestehende Variante ist mit dem Bibliothekskatalog oder einer SQLite-Begleitdatei verknüpft; bitte eine unabhängige Quelle verwenden".into());
    }
    folder.existing.reader.reserve_publication()?;
    folder.existing.reader.validate()?;
    for (i, r) in folder.existing.records.iter().enumerate() {
        duplicates::open_record(
            &folder.existing,
            r,
            Some(folder.existing.versions[i].as_ref().map_err(Clone::clone)?),
        )?;
    }
    let mut work = Work {
        control,
        started: Instant::now(),
        limits: LIMITS,
        charged: 0,
        bytes: 0,
        examined: 0,
        total: None,
        progress: |_| {},
        stage: "",
        last_progress: Instant::now(),
    };
    let current = inventory(&source, &mut work)?;
    if current.len() != folder.entries.len()
        || current
            .iter()
            .zip(&folder.entries)
            .any(|(a, b)| a.path != b.path || a.stamp != b.stamp)
    {
        return Err("Ordnerinhalt wurde seit der Vorbereitung verändert".into());
    }
    work.check()?;
    let mut library = super::Library::connect(root, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    folder.existing.reader.validate()?;
    let mut run = super::imports::ImportRun::new(control, progress);
    let bytes = folder.entries.iter().map(|e| e.size).sum();
    let count = folder.entries.len() as u64;
    if super::imports::available_space(root)? < super::imports::required_space(bytes, count)? {
        return Err("Nicht genug Speicherplatz für Paket und Reserve".into());
    }
    run.progress.total_bytes = bytes;
    run.progress.total_entries = count;
    run.progress.stage = "copying";
    run.report()?;
    let mut stage = Stage::create(&folder, &work)?;
    let mut result = (|| {
        let mut files = vec![];
        for entry in &folder.entries {
            run.check()?;
            prepared.pin.validate()?;
            work.check()?;
            stage.check_containers(&folder.existing)?;
            let parent = match entry.path.parent().filter(|p| !p.as_os_str().is_empty()) {
                Some(p) => anchored::relative(&stage.directory, p)?,
                None => stage.directory.try_clone().map_err(|e| e.to_string())?,
            };
            let name = entry
                .path
                .file_name()
                .ok_or("Dateiname fehlt")?
                .to_str()
                .ok_or("Dateiname ist kein UTF-8")?;
            if entry.directory {
                let dir = super::sprite_export::anchored::create_at_report_retained(
                    &parent,
                    name,
                    || control.mark_cleanup_failed(),
                )?;
                stage.outputs.push((
                    entry.path.clone(),
                    Stamp::read(&dir.metadata().map_err(|e| e.to_string())?),
                    true,
                ));
            } else {
                let mut input = anchored::relative(&source, &entry.path)?;
                control.check_source(&input)?;
                if Stamp::read(&input.metadata().map_err(|e| e.to_string())?) != entry.stamp {
                    return Err("Eingehende Datei wurde ersetzt".into());
                }
                let mut output = super::sprite_export::anchored::create_file(&parent, name)?;
                stage.outputs.push((
                    entry.path.clone(),
                    Stamp::read(&output.metadata().map_err(|e| e.to_string())?),
                    false,
                ));
                let mut sha = duplicates::sdk::Sha256::new();
                run.progress.current_path = Some(entry.path.to_str().unwrap().into());
                super::imports::copy_checked_into(
                    &mut input,
                    &mut output,
                    entry.size,
                    &mut run,
                    |chunk| sha.update(chunk),
                )?;
                if Some(sha.finish()) != entry.digest
                    || Stamp::read(&input.metadata().map_err(|e| e.to_string())?) != entry.stamp
                {
                    return Err(
                        "Tatsächlich kopierter Inhalt stimmt nicht mit der Vorbereitung überein"
                            .into(),
                    );
                }
                super::imports::record_asset(entry.path.clone(), entry.size, &mut files)?;
            }
            run.progress.completed_entries += 1;
            run.report()?;
        }
        let verified_outputs = stage.verify_outputs(&folder.entries, &mut work)?;
        prepared.pin.validate()?;
        let current = inventory(&source, &mut work)?;
        if current.len() != folder.entries.len()
            || current
                .iter()
                .zip(&folder.entries)
                .any(|(a, b)| a.path != b.path || a.stamp != b.stamp)
        {
            return Err("Ordnerinhalt wurde während des Kopierens verändert".into());
        }
        let name = prepared
            .source
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("Quellordner hat keinen Namen")?
            .to_owned();
        let source_path = super::path_text(&prepared.source)?;
        let relative = format!("packages/{}", stage.key);
        let mut operation = || {
            work.check()?;
            library.save_import_with(
                name.clone(),
                source_path.clone(),
                relative.clone(),
                files.clone(),
                &mut run,
                true,
                || {
                    work.check()?;
                    prepared.pin.validate()?;
                    prepared.destination.validate(root)?;
                    stage.validate(&folder.existing, &work)?;
                    let final_source = inventory(&source, &mut work)?;
                    if final_source.len() != folder.entries.len()
                        || final_source
                            .iter()
                            .zip(&folder.entries)
                            .any(|(a, b)| a.path != b.path || a.stamp != b.stamp)
                    {
                        return Err(
                            "Ordnerinhalt wurde unmittelbar vor dem Veröffentlichen verändert"
                                .into(),
                        );
                    }
                    if inventory(&stage.directory, &mut work)? != verified_outputs {
                        return Err(
                            "Kopierte Dateien wurden unmittelbar vor dem Veröffentlichen verändert"
                                .into(),
                        );
                    }
                    stage.publish()
                },
            )
        };
        publisher(&mut operation)
    })();
    if result.is_err() && !stage.cleanup() {
        control.mark_cleanup_failed();
        result = result.map_err(|e| {
            format!("{e}; der veränderte Zwischenordner wurde nicht vollständig entfernt")
        });
    }
    run.finish(&result);
    result
}
pub(crate) fn authorize(
    prepared: &PreparedPackage,
    root: &Path,
    control: &ImportControl,
) -> Result<Option<ZipEstimate>> {
    prepared.validate_selection()?;
    #[cfg(target_os = "macos")]
    prepared.destination.validate(root)?;
    if control
        .cancellation_flag()
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Err("Import abgebrochen".into());
    }
    if !prepared.zip {
        return Ok(None);
    }
    let mut callback = |_| {};
    let mut run = super::imports::ImportRun::new(control, &mut callback);
    #[cfg(target_os = "macos")]
    {
        let result = super::imports::estimate_open_zip(
            root,
            super::imports::open_zip_file(prepared.pin.validate()?)?,
            &mut run,
        )?;
        prepared.validate_selection()?;
        prepared.destination.validate(root)?;
        run.check()?;
        Ok(Some(result))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Vorbereiteter ZIP-Import ist derzeit nur für macOS qualifiziert".into())
    }
}
pub(crate) fn consume<'a>(
    root: &Path,
    prepared: PreparedPackage,
    authorized: bool,
    control: &'a ImportControl,
    progress: &'a mut dyn FnMut(ImportProgress),
    publisher: &'a mut super::imports::Publication<'a>,
) -> Result<AssetPackage> {
    prepared.validate_selection()?;
    #[cfg(target_os = "macos")]
    prepared.destination.validate(root)?;
    if control
        .cancellation_flag()
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Err("Import abgebrochen".into());
    }
    #[cfg(target_os = "macos")]
    {
        control.protect_sources(prepared.destination.protected.iter().copied())?;
        control.protect_sources(protected_catalog_sources(&prepared.destination.directory)?)?;
        if prepared.zip {
            control.check_source(&prepared.pin.validate()?)?;
        }
    }
    if prepared.verified() {
        #[cfg(target_os = "macos")]
        {
            return consume_folder(root, prepared, control, progress, publisher);
        }
    }
    if !authorized {
        return Err("Unvollständige Prüfung benötigt eine ausdrückliche Bestätigung des gesamten ungeprüften Pakets".into());
    }
    #[cfg(target_os = "macos")]
    let _catalog_publication =
        super::storage::reserve_catalog_publication(&prepared.destination.directory)?;
    // This is the explicitly authorized legacy full traversal, not bounded duplicate
    // inspection. Reject stable original sidecar aliases before opening a writer;
    // actual copied streams are checked again by the controlled folder importer.
    if !prepared.zip {
        let mut callback = |_| {};
        let run = super::imports::ImportRun::new(control, &mut callback);
        super::imports::folder_size(&prepared.source, &run)?;
        prepared.validate_selection()?;
        #[cfg(target_os = "macos")]
        prepared.destination.validate(root)?;
    }
    let mut library = super::Library::connect(root, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    prepared.validate_selection()?;
    let mut guarded_publisher = |operation: &mut dyn FnMut() -> Result<AssetPackage>| {
        prepared.validate_selection()?;
        #[cfg(target_os = "macos")]
        prepared.destination.validate(root)?;
        publisher(operation)
    };
    let mut guarded_progress = |value| {
        if prepared.validate_selection().is_err() {
            control.cancel();
        }
        #[cfg(target_os = "macos")]
        if prepared.destination.validate(root).is_err() {
            control.cancel();
        }
        progress(value)
    };
    if prepared.zip {
        #[cfg(target_os = "macos")]
        let reader = Some(prepared.pin.validate()?);
        #[cfg(not(target_os = "macos"))]
        let reader = None;
        library.import_zip_controlled(
            &prepared.source,
            control,
            &mut guarded_progress,
            Some(&mut guarded_publisher),
            reader,
        )
    } else {
        library.import_folder_controlled(
            &prepared.source,
            control,
            &mut guarded_progress,
            Some(&mut guarded_publisher),
        )
    }
}
