use super::*;
use std::{
    io::{Read, Write},
    sync::atomic::{AtomicBool, Ordering},
};
use zip::ZipArchive;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZipEstimate {
    pub uncompressed_bytes: u64,
    pub required_bytes: u64,
    pub available_bytes: u64,
    pub entry_count: usize,
    /// Unity packages only: payloads left out of a raw extraction (scripts, Unity YAML).
    pub skipped_entries: usize,
    pub has_space: bool,
}

#[derive(Default)]
pub struct ImportControl {
    pub(super) cancelled: AtomicBool,
}

impl ImportControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub stage: &'static str,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub completed_entries: u64,
    pub total_entries: u64,
    pub current_path: Option<String>,
}

pub(super) struct ImportRun<'a> {
    control: &'a ImportControl,
    on_progress: &'a mut dyn FnMut(ImportProgress),
    pub(super) progress: ImportProgress,
}

impl<'a> ImportRun<'a> {
    pub(super) fn new(
        control: &'a ImportControl,
        on_progress: &'a mut dyn FnMut(ImportProgress),
    ) -> Self {
        Self {
            control,
            on_progress,
            progress: ImportProgress {
                stage: "preparing",
                completed_bytes: 0,
                total_bytes: 0,
                completed_entries: 0,
                total_entries: 0,
                current_path: None,
            },
        }
    }

    pub(super) fn control(&self) -> &'a ImportControl {
        self.control
    }

    pub(super) fn check(&self) -> Result<()> {
        if self.control.cancelled.load(Ordering::Relaxed) {
            Err("Import abgebrochen; unvollständiges Paket wurde verworfen".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn report(&mut self) -> Result<()> {
        (self.on_progress)(self.progress.clone());
        self.check()
    }

    pub(super) fn cataloging(&mut self, path: &Path) -> Result<()> {
        self.progress.stage = "cataloging";
        self.progress.current_path = Some(path_text(path)?);
        self.report()
    }

    pub(super) fn finish(&mut self, result: &Result<AssetPackage>) {
        self.progress.stage = if result.is_ok() {
            "completed"
        } else if self.control.cancelled.load(Ordering::Relaxed) {
            "cancelled"
        } else {
            "failed"
        };
        self.progress.current_path = None;
        (self.on_progress)(self.progress.clone());
    }
}

impl Library {
    pub fn estimate_zip(&self, source: &Path) -> Result<ZipEstimate> {
        self.inspect_zip(
            source,
            &mut ImportRun::new(&ImportControl::default(), &mut |_| {}),
        )
    }

    pub(super) fn inspect_zip(
        &self,
        source: &Path,
        run: &mut ImportRun<'_>,
    ) -> Result<ZipEstimate> {
        if unity_packages::is_unity_path(source) {
            return unity_packages::estimate(&self.root, source, &run.control().cancelled);
        }
        estimate_open_zip(&self.root, open_zip(source)?, run)
    }

    pub fn import_folder(&mut self, source: &Path) -> Result<AssetPackage> {
        self.import_folder_with_progress(source, &ImportControl::default(), |_| {})
    }

    pub fn import_folder_with_progress(
        &mut self,
        source: &Path,
        control: &ImportControl,
        mut on_progress: impl FnMut(ImportProgress),
    ) -> Result<AssetPackage> {
        let mut run = ImportRun::new(control, &mut on_progress);
        let result = (|| {
            run.report()?;
            let source = source
                .canonical()
                .map_err(|e| format!("Quellordner nicht verfügbar: {e}"))?;
            if !source.is_dir() {
                return Err("Bitte einen Quellordner auswählen".into());
            }
            if source.starts_with(&self.root) || self.root.starts_with(&source) {
                return Err("Quellordner und Bibliothek dürfen nicht ineinander liegen".into());
            }
            let name = path_text(
                source
                    .file_name()
                    .map(Path::new)
                    .ok_or("Quellordner hat keinen Namen")?,
            )?;
            let (total_bytes, total_entries) = folder_size(&source, &run)?;
            if available_space(&self.root)? < required_space(total_bytes, total_entries)? {
                return Err("Nicht genug Speicherplatz für Paket und Reserve".into());
            }
            run.progress.total_bytes = total_bytes;
            run.progress.total_entries = total_entries;
            run.progress.stage = "copying";
            run.report()?;
            let (staging, relative_path) = self.prepare_import()?;
            let result = (|| {
                let mut files = Vec::new();
                copy_folder(&source, &staging, Path::new(""), &mut files, &mut run)?;
                self.finish_import(
                    name,
                    path_text(&source)?,
                    &staging,
                    relative_path.clone(),
                    files,
                    &mut run,
                )
            })();
            if result.is_err() {
                self.remove_failed_import(&staging, &relative_path);
            }
            result
        })();
        run.finish(&result);
        result
    }

    pub fn import_zip(&mut self, source: &Path) -> Result<AssetPackage> {
        self.import_zip_with_progress(source, &ImportControl::default(), |_| {})
    }

    pub fn import_zip_with_progress(
        &mut self,
        source: &Path,
        control: &ImportControl,
        mut on_progress: impl FnMut(ImportProgress),
    ) -> Result<AssetPackage> {
        let mut run = ImportRun::new(control, &mut on_progress);
        let result = (|| {
            run.report()?;
            let estimate = self.inspect_zip(source, &mut run)?;
            if !estimate.has_space {
                return Err("Nicht genug Speicherplatz für Archivinhalt und Reserve".into());
            }
            let source = source
                .canonical()
                .map_err(|e| format!("Archiv nicht verfügbar: {e}"))?;
            let name = path_text(
                source
                    .file_stem()
                    .map(Path::new)
                    .ok_or("Archiv hat keinen Namen")?,
            )?;
            run.progress.total_bytes = estimate.uncompressed_bytes;
            run.progress.total_entries = estimate.entry_count as u64;
            run.progress.stage = "copying";
            run.report()?;
            let (staging, relative_path) = self.prepare_import()?;
            let result = (|| {
                let files = if unity_packages::is_unity_path(&source) {
                    unity_packages::extract(&source, &staging, &mut run)?
                } else {
                    extract_zip(&source, &staging, estimate.entry_count, &mut run)?
                };
                self.finish_import(
                    name,
                    path_text(&source)?,
                    &staging,
                    relative_path.clone(),
                    files,
                    &mut run,
                )
            })();
            if result.is_err() {
                self.remove_failed_import(&staging, &relative_path);
            }
            result
        })();
        run.finish(&result);
        result
    }

    pub(super) fn prepare_import(&self) -> Result<(PathBuf, String)> {
        let packages = self.root.join("packages");
        if packages.exists()
            && fs::symlink_metadata(&packages)
                .map_err(|e| format!("Paketspeicher nicht verfügbar: {e}"))?
                .file_type()
                .is_symlink()
        {
            return Err("Der Paketspeicher darf keine symbolische Verknüpfung sein".into());
        }
        fs::create_dir_all(&packages)
            .map_err(|e| format!("Paketspeicher kann nicht erstellt werden: {e}"))?;
        let key = unique_suffix();
        let staging = packages.join(format!(".import-{key}"));
        fs::create_dir(&staging)
            .map_err(|e| format!("Import kann nicht vorbereitet werden: {e}"))?;
        Ok((staging, format!("packages/{key}")))
    }

    fn finish_import(
        &mut self,
        name: String,
        source_path: String,
        staging: &Path,
        relative_path: String,
        files: Vec<ImportedFile>,
        run: &mut ImportRun<'_>,
    ) -> Result<AssetPackage> {
        run.check()?;
        fs::rename(staging, self.root.join(&relative_path))
            .map_err(|e| format!("Paket kann nicht gespeichert werden: {e}"))?;
        self.save_import(name, source_path, relative_path, files, run)
    }

    fn remove_failed_import(&self, staging: &Path, relative_path: &str) {
        for path in [staging.to_owned(), self.root.join(relative_path)] {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn extract_zip(
    source: &Path,
    staging: &Path,
    entry_count: usize,
    run: &mut ImportRun<'_>,
) -> Result<Vec<ImportedFile>> {
    let mut archive = open_zip(source)?;
    if archive.len() != entry_count {
        return Err("ZIP-Datei wurde während des Imports verändert".into());
    }
    let mut files = Vec::new();
    for index in 0..archive.len() {
        run.check()?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
        let path = checked_zip_path(&entry)?;
        run.progress.current_path = Some(path_text(&path)?);
        if entry.is_dir() {
            create_safe_directories(staging, &path)?;
        } else {
            create_safe_directories(staging, path.parent().unwrap_or(Path::new("")))?;
            let size = entry.size();
            copy_checked(&mut entry, &staging.join(&path), size, run)?;
            record_asset(path, size, &mut files)?;
        }
        run.progress.completed_entries += 1;
        run.report()?;
    }
    Ok(files)
}

fn estimate_open_zip(
    root: &Path,
    mut archive: ZipArchive<fs::File>,
    run: &mut ImportRun<'_>,
) -> Result<ZipEstimate> {
    let mut uncompressed_bytes = 0_u64;
    for index in 0..archive.len() {
        run.check()?;
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
        checked_zip_path(&entry)?;
        if !entry.is_dir() {
            uncompressed_bytes = uncompressed_bytes
                .checked_add(entry.size())
                .ok_or("ZIP ist zu groß")?;
        }
    }
    let required_bytes = required_space(uncompressed_bytes, archive.len() as u64)?;
    let available_bytes = available_space(root)?;
    Ok(ZipEstimate {
        uncompressed_bytes,
        required_bytes,
        available_bytes,
        entry_count: archive.len(),
        skipped_entries: 0,
        has_space: available_bytes >= required_bytes,
    })
}

pub(super) fn open_zip(source: &Path) -> Result<ZipArchive<fs::File>> {
    let file = fs::File::open(source)
        .map_err(|e| format!("ZIP-Datei nicht verfügbar oder nicht lokal lesbar: {e}"))?;
    open_zip_file(file)
}
fn open_zip_file(file: fs::File) -> Result<ZipArchive<fs::File>> {
    if !file
        .metadata()
        .map_err(|e| format!("ZIP-Datei nicht verfügbar: {e}"))?
        .is_file()
    {
        return Err("Bitte eine reguläre ZIP-Datei auswählen".into());
    }
    ZipArchive::new(file)
        .map_err(|e| format!("ZIP-Datei ist beschädigt oder nicht unterstützt: {e}"))
}

pub(super) fn checked_zip_path(entry: &zip::read::ZipFile<'_, fs::File>) -> Result<PathBuf> {
    let path = entry.enclosed_name().ok_or("Unsicherer Dateipfad im ZIP")?;
    if path.as_os_str().is_empty()
        // '\\' (UNC, Windows separators) and ':' (drive letters, NTFS streams) on every OS,
        // so one archive is judged the same everywhere.
        || entry.name().contains(['\\', ':'])
        || Path::new(entry.name())
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("Unsicherer Dateipfad im ZIP: {}", entry.name()));
    }
    let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
    if entry.is_symlink() || (kind != 0 && kind != 0o100000 && kind != 0o040000) {
        return Err(format!(
            "Verknüpfungen und spezielle Dateien werden nicht importiert: {}",
            entry.name()
        ));
    }
    Ok(path)
}

pub(super) fn create_safe_directories(staging: &Path, relative: &Path) -> Result<()> {
    let mut path = staging.to_owned();
    for part in relative.components() {
        path.push(part);
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|e| format!("Paketordner nicht verfügbar: {e}"))?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("ZIP-Dateipfade kollidieren".into());
                }
            }
            Err(error) => return Err(format!("Paketordner kann nicht erstellt werden: {error}")),
        }
    }
    Ok(())
}

pub(super) fn required_space(bytes: u64, entries: u64) -> Result<u64> {
    let overhead = entries
        .checked_mul(4096)
        .ok_or("Paket enthält zu viele Einträge")?;
    let reserve = (bytes / 10).max(64 * 1024 * 1024);
    bytes
        .checked_add(overhead)
        .and_then(|size| size.checked_add(reserve))
        .ok_or_else(|| "Paket ist zu groß".into())
}

#[cfg(windows)]
pub(super) use super::volume::available_space;

#[cfg(not(windows))]
pub(super) fn available_space(root: &Path) -> Result<u64> {
    let output = std::process::Command::new("/bin/df")
        .arg("-Pk")
        .arg(root)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("Freier Speicherplatz kann nicht ermittelt werden: {e}"))?;
    if !output.status.success() {
        return Err("Freier Speicherplatz kann nicht ermittelt werden".into());
    }
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| "Speicherplatzangabe ist nicht lesbar")?;
    text.lines()
        .last()
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|value| value.checked_mul(1024))
        .ok_or_else(|| "Speicherplatzangabe ist nicht lesbar".into())
}

#[derive(Clone)]
pub(super) struct ImportedFile {
    pub(super) relative: PathBuf,
    pub(super) size: i64,
    pub(super) format: String,
}

pub(super) fn record_asset(path: PathBuf, size: u64, files: &mut Vec<ImportedFile>) -> Result<()> {
    if let Some(format) = assets::candidate_format(&path) {
        files.push(ImportedFile {
            relative: path,
            size: i64::try_from(size).map_err(|_| "Datei ist zu groß für den Katalog")?,
            format: format.into(),
        });
    }
    Ok(())
}

pub(super) fn folder_size(source: &Path, run: &ImportRun<'_>) -> Result<(u64, u64)> {
    let (mut bytes, mut entries) = (0_u64, 0_u64);
    for entry in fs::read_dir(source).map_err(|e| format!("Quellordner nicht lesbar: {e}"))? {
        run.check()?;
        let entry = entry.map_err(|e| format!("Quelle nicht lesbar: {e}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|e| format!("Quelldatei nicht lesbar: {e}"))?;
        entries = entries
            .checked_add(1)
            .ok_or("Paket enthält zu viele Einträge")?;
        if metadata.file_type().is_symlink() {
            return Err("Symbolische Verknüpfung wird nicht importiert".into());
        }
        if metadata.is_dir() {
            let (child_bytes, child_entries) = folder_size(&entry.path(), run)?;
            bytes = bytes.checked_add(child_bytes).ok_or("Paket ist zu groß")?;
            entries = entries
                .checked_add(child_entries)
                .ok_or("Paket enthält zu viele Einträge")?;
        } else if metadata.is_file() {
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or("Paket ist zu groß")?;
        } else {
            return Err("Nur reguläre Dateien werden importiert".into());
        }
    }
    Ok((bytes, entries))
}

fn copy_folder(
    source: &Path,
    destination: &Path,
    relative: &Path,
    files: &mut Vec<ImportedFile>,
    run: &mut ImportRun<'_>,
) -> Result<()> {
    for entry in fs::read_dir(source).map_err(|e| format!("Quellordner nicht lesbar: {e}"))? {
        run.check()?;
        let entry = entry.map_err(|e| format!("Quelle nicht lesbar: {e}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|e| format!("Quelldatei nicht lesbar: {e}"))?;
        let target = destination.join(entry.file_name());
        let path = relative.join(entry.file_name());
        run.progress.current_path = Some(path_text(&path)?);
        if metadata.file_type().is_symlink() {
            return Err("Symbolische Verknüpfung wird nicht importiert".into());
        }
        if metadata.is_dir() {
            fs::create_dir(&target)
                .map_err(|e| format!("Paketordner kann nicht erstellt werden: {e}"))?;
            run.progress.completed_entries += 1;
            run.report()?;
            copy_folder(&entry.path(), &target, &path, files, run)?;
        } else if metadata.is_file() {
            let mut input = fs::File::open(entry.path())
                .map_err(|e| format!("Quelldatei nicht lesbar: {e}"))?;
            copy_checked(&mut input, &target, metadata.len(), run)?;
            record_asset(path, metadata.len(), files)?;
            run.progress.completed_entries += 1;
            run.report()?;
        } else {
            return Err("Nur reguläre Dateien werden importiert".into());
        }
    }
    Ok(())
}

pub(super) fn copy_checked(
    input: &mut impl Read,
    target: &Path,
    expected: u64,
    run: &mut ImportRun<'_>,
) -> Result<()> {
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| {
            format!(
                "Dateipfad kollidiert oder ist nicht schreibbar ({}): {e}",
                target.display()
            )
        })?;
    copy_checked_into(input, &mut output, expected, run, |_| {})
}
pub(super) fn copy_checked_into(
    input: &mut impl Read,
    output: &mut fs::File,
    expected: u64,
    run: &mut ImportRun<'_>,
    mut copied: impl FnMut(&[u8]),
) -> Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    let mut actual = 0_u64;
    loop {
        run.check()?;
        let count = input
            .read(&mut buffer)
            .map_err(|e| format!("Datei kann nicht gelesen oder entpackt werden: {e}"))?;
        if count == 0 {
            break;
        }
        actual = actual
            .checked_add(count as u64)
            .ok_or("Datei ist zu groß")?;
        if actual > expected {
            return Err("Tatsächliche Dateigröße überschreitet die erwartete Größe".into());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| format!("Datei kann nicht gespeichert werden: {e}"))?;
        copied(&buffer[..count]);
        run.progress.completed_bytes = run
            .progress
            .completed_bytes
            .checked_add(count as u64)
            .ok_or("Paket ist zu groß")?;
        if run.progress.completed_bytes > run.progress.total_bytes {
            return Err("Paketgröße wurde während des Imports verändert".into());
        }
        run.report()?;
    }
    if actual != expected {
        return Err("Dateigröße stimmt nicht mit der erwarteten Größe überein".into());
    }
    output
        .flush()
        .map_err(|e| format!("Datei kann nicht gespeichert werden: {e}"))?;
    Ok(())
}
