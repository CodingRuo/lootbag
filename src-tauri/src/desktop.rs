use crate::library::{
    AssetPackage, AudioCriteria, AudioMetadata, AudioStatus, AudioWaveform, BulkAction, BulkResult,
    BulkSelection, CatalogFilters, CatalogPage, CatalogQuery, DuplicateCopy, EmbeddedAudioTags,
    EmptiedTrash, ExportOutcome, ImageGroup, ImageWorkflow, ImportControl, ImportProgress,
    KindCounts, Library, ModelResources, PackageInfo, PackageInfoDetails, ProjectExportPlan,
    ProjectFolder, Result, SavedSearch, TrashEntry, ZipEstimate,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, PoisonError, TryLockError,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
struct Session {
    root: Mutex<Option<PathBuf>>,
    import_gate: Mutex<()>,
    import_control: Mutex<Option<Arc<ImportControl>>>,
    model_preview: Mutex<PreviewState>,
    audio_waveform: Mutex<Option<(String, Arc<AtomicBool>)>>,
    audio_gate: Mutex<()>,
    audio_tags: Mutex<Option<(String, Arc<AtomicBool>)>>,
    audio_tags_gate: Mutex<()>,
    sprite_export: Mutex<Option<(String, Arc<AtomicBool>)>>,
    audio_analysis: Mutex<Option<AudioAnalysisJob>>,
    model_traits: Mutex<Option<Arc<AtomicBool>>>,
}

struct AudioAnalysisJob {
    request_id: String,
    cancelled: Arc<AtomicBool>,
    files: Option<Vec<i64>>,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedAudioAnalysis {
    request_id: String,
    total: usize,
}
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioAnalysisProgress {
    request_id: String,
    total: usize,
    processed: usize,
    ready: usize,
    errors: usize,
    skipped: usize,
    cancelled: bool,
}

#[derive(Default)]
struct PreviewState {
    request: u64,
    snapshot: Option<ModelResources>,
}

impl Session {
    fn begin_audio_analysis(&self, request_id: &str) -> Result<(PathBuf, Arc<AtomicBool>)> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err("Ungültige Audioanalyse-Anfrage".into());
        }
        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        let root = current
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let flag = Arc::new(AtomicBool::new(false));
        let mut job = self
            .audio_analysis
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if job.as_ref().is_some_and(|job| job.request_id == request_id) {
            return Err(
                "Audioanalyse-ID ist bereits aktiv. Bitte eine neue Anfrage-ID verwenden".into(),
            );
        }
        if let Some(older) = job.replace(AudioAnalysisJob {
            request_id: request_id.into(),
            cancelled: flag.clone(),
            files: None,
        }) {
            older.cancelled.store(true, Ordering::Relaxed);
        }
        Ok((root, flag))
    }
    fn finish_audio_preparation(
        &self,
        root: &Path,
        request_id: &str,
        flag: &Arc<AtomicBool>,
        files: Vec<i64>,
    ) -> Result<PreparedAudioAnalysis> {
        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        let mut jobs = self
            .audio_analysis
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let job = jobs
            .as_mut()
            .filter(|job| job.request_id == request_id && Arc::ptr_eq(&job.cancelled, flag))
            .ok_or("Audioanalyse-Anfrage wurde ersetzt")?;
        if current.as_deref() != Some(root) || flag.load(Ordering::Relaxed) {
            return Err("Audioanalyse abgebrochen".into());
        }
        let total = files.len();
        job.files = Some(files);
        Ok(PreparedAudioAnalysis {
            request_id: request_id.into(),
            total,
        })
    }
    fn take_audio_analysis(
        &self,
        request_id: &str,
    ) -> Result<(PathBuf, Arc<AtomicBool>, Vec<i64>)> {
        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        let root = current
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let mut jobs = self
            .audio_analysis
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let job = jobs
            .as_mut()
            .filter(|job| job.request_id == request_id && !job.cancelled.load(Ordering::Relaxed))
            .ok_or("Bitte den Sound-Suchbereich erneut vorbereiten")?;
        let files = job
            .files
            .take()
            .ok_or("Audioanalyse wurde noch nicht vorbereitet oder läuft bereits")?;
        Ok((root, job.cancelled.clone(), files))
    }
    fn cancel_audio_analysis(&self, request_id: Option<&str>) -> Result<()> {
        let mut jobs = self
            .audio_analysis
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if jobs
            .as_ref()
            .is_some_and(|job| request_id.is_none_or(|id| job.request_id == id))
        {
            if let Some(job) = jobs.take() {
                job.cancelled.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }
    fn audio_analysis_is_current(&self, request_id: &str, flag: &Arc<AtomicBool>) -> Result<bool> {
        let jobs = self
            .audio_analysis
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(!flag.load(Ordering::Relaxed)
            && jobs.as_ref().is_some_and(|job| {
                job.request_id == request_id && Arc::ptr_eq(&job.cancelled, flag)
            }))
    }
    fn finish_audio_analysis(&self, request_id: &str, flag: &Arc<AtomicBool>) -> Result<()> {
        let mut jobs = self
            .audio_analysis
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if jobs
            .as_ref()
            .is_some_and(|job| job.request_id == request_id && Arc::ptr_eq(&job.cancelled, flag))
        {
            jobs.take();
        }
        Ok(())
    }
    fn run_prepared_audio(
        &self,
        library: &Library,
        root: &Path,
        request_id: &str,
        flag: &Arc<AtomicBool>,
        files: Vec<i64>,
        mut progress: impl FnMut(AudioAnalysisProgress),
    ) -> Result<AudioAnalysisProgress> {
        let result = (|| {
            let _gate = file_gate(&self.import_gate)?;
            library.audio_analysis_busy_timeout()?;
            let mut summary = AudioAnalysisProgress {
                request_id: request_id.into(),
                total: files.len(),
                processed: 0,
                ready: 0,
                errors: 0,
                skipped: 0,
                cancelled: false,
            };
            progress(summary.clone());
            for file_id in files {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let measured = library.inspect_audio(file_id, flag);
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let stored = match measured {
                    Ok(measured) => {
                        // Root ownership is held only for a bounded per-file commit; cancellation takes no root lock.
                        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
                        if current.as_deref() != Some(root)
                            || !self.audio_analysis_is_current(request_id, flag)?
                        {
                            break;
                        }
                        library.store_audio_inspection(file_id, measured, flag)
                    }
                    Err(error) => Err(error),
                };
                if flag.load(Ordering::Relaxed) && stored.is_err() {
                    break;
                }
                match stored {
                    Ok(metadata) if metadata.status == AudioStatus::Ready => summary.ready += 1,
                    Ok(_) => summary.errors += 1,
                    Err(_) => summary.skipped += 1,
                }
                summary.processed += 1;
                progress(summary.clone());
            }
            summary.cancelled = flag.load(Ordering::Relaxed)
                || !self.audio_analysis_is_current(request_id, flag)?
                || self
                    .root
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_deref()
                    != Some(root);
            progress(summary.clone());
            Ok(summary)
        })();
        self.finish_audio_analysis(request_id, flag)?;
        result
    }

    fn begin_audio_tags(&self, request_id: &str) -> Result<(PathBuf, Arc<AtomicBool>)> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err("Ungültige Audio-Tag-Anfrage".into());
        }
        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        let root = current
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut job = self
            .audio_tags
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((_, older)) = job.replace((request_id.into(), cancelled.clone())) {
            older.store(true, Ordering::Relaxed);
        }
        Ok((root, cancelled))
    }
    fn finish_audio_tags(
        &self,
        root: &Path,
        request_id: &str,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        let mut job = self
            .audio_tags
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let matches = job
            .as_ref()
            .is_some_and(|(id, flag)| id == request_id && Arc::ptr_eq(flag, cancelled));
        if matches {
            job.take();
        }
        if !matches || current.as_deref() != Some(root) || cancelled.load(Ordering::Relaxed) {
            return Err("Audio-Tag-Analyse abgebrochen".into());
        }
        Ok(())
    }
    fn cancel_audio_tags(&self, request_id: Option<&str>) -> Result<()> {
        let mut job = self
            .audio_tags
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if job
            .as_ref()
            .is_some_and(|(id, _)| request_id.is_none_or(|request_id| request_id == id))
        {
            if let Some((_, flag)) = job.take() {
                flag.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    fn cancel_sprite_export(&self, snapshot_id: Option<&str>) -> Result<()> {
        let current = self
            .sprite_export
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if current
            .as_ref()
            .is_some_and(|(id, _)| snapshot_id.is_none_or(|requested| requested == id))
        {
            if let Some((_, cancelled)) = current.as_ref() {
                cancelled.store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    }
    fn with_sprite_preview<T>(
        &self,
        snapshot_id: &str,
        file_id: i64,
        publish: impl FnOnce(&Path, &ModelResources) -> Result<T>,
    ) -> Result<T> {
        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        let preview = self
            .model_preview
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let snapshot = preview
            .snapshot
            .as_ref()
            .ok_or("Bitte die Bildvorschau vor dem Export öffnen")?;
        if snapshot.snapshot_id != snapshot_id || snapshot.file_id != file_id {
            return Err("Sprite-Vorschau wurde inzwischen gewechselt".into());
        }
        publish(
            current
                .as_deref()
                .ok_or("Bitte zuerst eine Bibliothek öffnen")?,
            snapshot,
        )
    }
    fn set_root(&self, root: PathBuf) -> Result<()> {
        self.cancel_sprite_export(None)?;
        let mut current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        let mut preview = self
            .model_preview
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *current = Some(root);
        preview.request = preview.request.wrapping_add(1);
        preview.snapshot = None;
        if let Some((_, cancelled)) = self
            .audio_waveform
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            cancelled.store(true, Ordering::Relaxed);
        }
        self.cancel_audio_tags(None)?;
        self.cancel_audio_analysis(None)?;
        self.replace_model_traits(None);
        Ok(())
    }

    /// Cancels the running background inspection and makes `next` the current one.
    fn replace_model_traits(&self, next: Option<Arc<AtomicBool>>) {
        let mut current = self
            .model_traits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = std::mem::replace(&mut *current, next) {
            previous.store(true, Ordering::Relaxed);
        }
    }

    fn begin_preview(&self) -> Result<(PathBuf, u64)> {
        self.cancel_sprite_export(None)?;
        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        let root = current
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let mut preview = self
            .model_preview
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        preview.request = preview.request.wrapping_add(1);
        preview.snapshot = None;
        Ok((root, preview.request))
    }

    fn pin_preview(&self, root: &Path, request: u64, snapshot: &ModelResources) -> Result<()> {
        let current = self.root.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_deref() != Some(root) {
            return Err("Die Bibliothek wurde während der Vorschau-Vorbereitung gewechselt".into());
        }
        let mut preview = self
            .model_preview
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if preview.request != request {
            return Err("Die Vorschau wurde während der Vorbereitung gewechselt".into());
        }
        preview.snapshot = Some(snapshot.clone());
        Ok(())
    }

    fn export_preview(&self, snapshot_id: &str, file_id: i64) -> Result<ModelResources> {
        let snapshot = self
            .model_preview
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot
            .clone()
            .ok_or("Bitte die Vorschau vor dem Export öffnen")?;
        if snapshot.snapshot_id != snapshot_id || snapshot.file_id != file_id {
            return Err(
                "Die Vorschau wurde inzwischen gewechselt. Bitte die Vorschau erneut öffnen".into(),
            );
        }
        Ok(snapshot)
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryInfo {
    root: String,
    name: String,
    packages: Vec<AssetPackage>,
    folders: Vec<ProjectFolder>,
    asset_count: i64,
    kind_counts: KindCounts,
}

fn info(library: &Library) -> Result<LibraryInfo> {
    let packages = library.packages()?;
    Ok(LibraryInfo {
        root: library.root().to_string_lossy().into_owned(),
        name: library
            .root()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        asset_count: packages.iter().map(|package| package.asset_count).sum(),
        kind_counts: library.kind_counts()?,
        packages,
        folders: library.folders()?,
    })
}

fn active_root(app: &AppHandle) -> Result<PathBuf> {
    app.state::<Session>()
        .root
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or_else(|| "Bitte zuerst eine Bibliothek öffnen".into())
}

/// Serializes catalog and file writes. A panic in an earlier holder must not block the app.
fn file_gate(gate: &Mutex<()>) -> Result<MutexGuard<'_, ()>> {
    match gate.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::Poisoned(poisoned)) => Ok(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => Err("Bitte den laufenden Dateivorgang abwarten".into()),
    }
}

/// The file gate, held while `root` is still the active library.
fn catalog_write<'a>(app: &'a AppHandle, root: &Path) -> Result<MutexGuard<'a, ()>> {
    let gate = file_gate(&app.state::<Session>().inner().import_gate)?;
    if active_root(app)? != root {
        return Err("Die aktive Bibliothek wurde geändert".into());
    }
    Ok(gate)
}

const LAST_LIBRARY: &str = "last-library";

#[derive(Clone, serde::Serialize)]
struct BackfillProgress {
    done: usize,
    remaining: usize,
    /// Hashes dropped because their file changed; a run may finish with only these.
    invalidated: usize,
}

/// One backfill batch: (processed, still pending, hashes dropped as stale).
type Batch = (usize, usize, usize);

/// Derives auto-tags from names, classifies not yet inspected model files of `root`, then hashes
/// files that may be duplicates, on a worker thread that replaces an earlier run. Emits
/// `auto-tags-progress`, `model-traits-progress` and `file-hashes-progress` after each batch.
fn start_backfill(app: &AppHandle, root: PathBuf) {
    let cancelled = Arc::new(AtomicBool::new(false));
    app.state::<Session>()
        .replace_model_traits(Some(cancelled.clone()));
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(library) = Library::writable(&root) else {
            return;
        };
        let run = |event: &str, step: &dyn Fn(&mut i64) -> Result<Batch>| {
            let (mut after, mut done, mut invalidated) = (0, 0, 0);
            while !cancelled.load(Ordering::Relaxed) {
                match step(&mut after) {
                    Ok((processed, remaining, dropped)) => {
                        done += processed;
                        invalidated += dropped;
                        if !cancelled.load(Ordering::Relaxed) {
                            let progress = BackfillProgress {
                                done,
                                remaining,
                                invalidated,
                            };
                            let _ = app.emit(event, progress);
                        }
                        if remaining == 0 {
                            break;
                        }
                    }
                    Err(error) => {
                        // Retried on the next activation or import.
                        eprintln!("{error}");
                        break;
                    }
                }
            }
        };
        run("auto-tags-progress", &|after| {
            library
                .tag_pending_assets(after, 500, &cancelled)
                .map(|(done, remaining)| (done, remaining, 0))
        });
        run("model-traits-progress", &|after| {
            library
                .inspect_pending_models(after, 200, &cancelled)
                .map(|(done, remaining)| (done, remaining, 0))
        });
        run("file-hashes-progress", &|after| {
            library.hash_pending_files(after, 100, &cancelled)
        });
    });
}

/// Grants the asset protocol whole source roots once instead of one entry per previewed file.
fn allow_root(app: &AppHandle, root: &Path) -> Result<()> {
    app.asset_protocol_scope()
        .allow_directory(root, true)
        .map_err(|error| format!("Bibliothek kann nicht freigegeben werden: {error}"))
}

/// Makes `library` the active one and remembers it for the next launch.
fn activate(app: &AppHandle, library: &Library) -> Result<LibraryInfo> {
    let result = info(library)?;
    allow_root(app, library.root())?;
    for source in library.external_roots()? {
        // An offline source is granted again on the next activation or relink.
        let _ = allow_root(app, &source);
    }
    app.state::<Session>().set_root(library.root().to_owned())?;
    crate::library::forget_verified_zip_cache();
    start_backfill(app, library.root().to_owned());
    if let Ok(dir) = app.path().app_config_dir() {
        let _ = fs::create_dir_all(&dir).and_then(|()| {
            fs::write(
                dir.join(LAST_LIBRARY),
                library.root().to_string_lossy().as_bytes(),
            )
        });
    }
    Ok(result)
}

fn selected_path(app: &AppHandle, path: &Path) -> Result<()> {
    if !path.is_absolute() || !app.asset_protocol_scope().is_allowed(path) {
        return Err("Bitte den Speicherort im nativen Dialog auswählen".into());
    }
    Ok(())
}

async fn background<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| format!("Dateivorgang konnte nicht abgeschlossen werden: {error}"))?
}

#[tauri::command]
async fn create_library(app: AppHandle, root: Option<String>) -> Result<LibraryInfo> {
    let chosen = root.map(PathBuf::from);
    if let Some(root) = &chosen {
        selected_path(&app, root)?;
    }
    let home = app
        .path()
        .home_dir()
        .map_err(|error| format!("Standardordner nicht verfügbar: {error}"))?;
    background(move || {
        let _gate = file_gate(&app.state::<Session>().inner().import_gate)?;
        let library = match chosen {
            Some(root) => Library::create(&root)?,
            None => Library::create_default(&home)?,
        };
        activate(&app, &library)
    })
    .await
}

#[tauri::command]
async fn open_library(app: AppHandle, root: String) -> Result<LibraryInfo> {
    let root = PathBuf::from(root);
    selected_path(&app, &root)?;
    background(move || {
        let _gate = file_gate(&app.state::<Session>().inner().import_gate)?;
        activate(&app, &Library::open(&root)?)
    })
    .await
}

#[tauri::command]
async fn get_library(app: AppHandle) -> Result<Option<LibraryInfo>> {
    let root = app
        .state::<Session>()
        .root
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    background(move || {
        root.map(|root| Library::writable(&root).and_then(|library| info(&library)))
            .transpose()
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportSource {
    path: String,
    is_zip: bool,
}

fn describe_import_source(source: String) -> Result<ImportSource> {
    let path = Path::new(&source);
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Importquelle nicht verfügbar: {error}"))?;
    let is_zip = metadata.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    if !metadata.is_dir() && !is_zip {
        return Err("Bitte einen Ordner oder eine ZIP-Datei auswählen".into());
    }
    Ok(ImportSource {
        path: source,
        is_zip,
    })
}

#[tauri::command]
async fn dropped_import_sources(app: AppHandle, sources: Vec<String>) -> Result<Vec<ImportSource>> {
    active_root(&app)?;
    if sources.is_empty() || sources.len() > 32 {
        return Err("Bitte 1 bis 32 Ordner oder ZIP-Pakete gleichzeitig ablegen".into());
    }
    for source in &sources {
        selected_path(&app, Path::new(source))?;
    }
    // Tauri grants native dropped paths before emitting the drop event; never grant IPC paths here.
    background(move || sources.into_iter().map(describe_import_source).collect()).await
}

#[cfg(target_os = "macos")]
extern "C" {
    fn select_import_path(parent: *mut std::ffi::c_void) -> *mut std::ffi::c_char;
    fn release_import_path(path: *mut std::ffi::c_char);
}

#[tauri::command]
async fn select_import_source(
    app: AppHandle,
    window: tauri::Window,
) -> Result<Option<ImportSource>> {
    active_root(&app)?;
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = std::sync::mpsc::channel();
        let picker_app = app.clone();
        app.run_on_main_thread(move || {
            let result = (|| {
                let parent = window.ns_window().map_err(|error| error.to_string())?;
                // SAFETY: the SDK dialog runs on the main thread with a live parent window.
                // Its returned UTF-8 path is malloc-owned and released by the matching C function.
                let path = unsafe { select_import_path(parent) };
                if path.is_null() {
                    return Ok(None);
                }
                let source = unsafe { std::ffi::CStr::from_ptr(path) }
                    .to_str()
                    .map(str::to_owned)
                    .map_err(|_| "Der ausgewählte Pfad enthält ungültiges UTF-8".to_string());
                unsafe { release_import_path(path) };
                let selected = describe_import_source(source?)?;
                let path = Path::new(&selected.path);
                let scope = picker_app.asset_protocol_scope();
                if selected.is_zip {
                    scope.allow_file(path)
                } else {
                    scope.allow_directory(path, false)
                }
                .map_err(|error| format!("Importquelle nicht freigegeben: {error}"))?;
                Ok(Some(selected))
            })();
            let _ = sender.send(result);
        })
        .map_err(|error| format!("Dateiauswahl nicht verfügbar: {error}"))?;
        background(move || receiver.recv().map_err(|error| error.to_string())?).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Err("Die gemeinsame Paketauswahl ist derzeit für macOS verfügbar".into())
    }
}

#[tauri::command]
async fn import_folder(
    app: AppHandle,
    source: String,
    on_progress: tauri::ipc::Channel<ImportProgress>,
) -> Result<AssetPackage> {
    managed_import(app, source, on_progress, false).await
}

#[tauri::command]
async fn import_zip(
    app: AppHandle,
    source: String,
    on_progress: tauri::ipc::Channel<ImportProgress>,
) -> Result<AssetPackage> {
    managed_import(app, source, on_progress, true).await
}

#[tauri::command]
async fn estimate_zip(app: AppHandle, source: String) -> Result<ZipEstimate> {
    let source = PathBuf::from(source);
    selected_path(&app, &source)?;
    let root = active_root(&app)?;
    background(move || Library::writable(&root)?.estimate_zip(&source)).await
}

#[tauri::command]
fn cancel_import(app: AppHandle) -> Result<bool> {
    let state = app.state::<Session>();
    let control = state
        .import_control
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(control) = control.as_ref() {
        control.cancel();
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn managed_import(
    app: AppHandle,
    source: String,
    on_progress: tauri::ipc::Channel<ImportProgress>,
    is_zip: bool,
) -> Result<AssetPackage> {
    let source = PathBuf::from(source);
    selected_path(&app, &source)?;
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        let state = app.state::<Session>();
        let control = Arc::new(ImportControl::default());
        *state
            .import_control
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(control.clone());
        let mut last_report = None;
        let mut last_stage = "";
        let progress = |progress: ImportProgress| {
            let now = Instant::now();
            if last_stage != progress.stage
                || last_report
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(100))
            {
                last_stage = progress.stage;
                last_report = Some(now);
                let _ = on_progress.send(progress);
            }
        };
        let result = Library::writable(&root).and_then(|mut library| {
            if is_zip {
                library.import_zip_with_progress(&source, &control, progress)
            } else {
                library.import_folder_with_progress(&source, &control, progress)
            }
        });
        *state
            .import_control
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        if result.is_ok() {
            start_backfill(&app, root.clone());
        }
        result
    })
    .await
}

#[tauri::command]
async fn reference_folder(app: AppHandle, source: String) -> Result<AssetPackage> {
    let source = PathBuf::from(source);
    selected_path(&app, &source)?;
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        let package = Library::writable(&root)?.reference_folder(&source)?;
        allow_root(
            &app,
            Path::new(package.external_root.as_deref().unwrap_or_default()),
        )?;
        start_backfill(&app, root.clone());
        Ok(package)
    })
    .await
}

#[tauri::command]
async fn reference_zip(app: AppHandle, source: String) -> Result<AssetPackage> {
    let source = PathBuf::from(source);
    selected_path(&app, &source)?;
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        // Entries are served from the library's cache, so the archive itself needs no scope.
        let package = Library::writable(&root)?.reference_zip(&source)?;
        start_backfill(&app, root.clone());
        Ok(package)
    })
    .await
}

#[tauri::command]
async fn relink_source(app: AppHandle, package_id: i64, source: String) -> Result<AssetPackage> {
    let source = PathBuf::from(source);
    selected_path(&app, &source)?;
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        let package = Library::writable(&root)?.relink_source(package_id, &source)?;
        allow_root(
            &app,
            Path::new(package.external_root.as_deref().unwrap_or_default()),
        )?;
        start_backfill(&app, root.clone());
        Ok(package)
    })
    .await
}

#[tauri::command]
async fn query_catalog(
    app: AppHandle,
    query: CatalogQuery,
    audio_criteria: Option<AudioCriteria>,
) -> Result<CatalogPage> {
    let root = active_root(&app)?;
    background(move || Library::writable(&root)?.query_with_audio(query, audio_criteria)).await
}

/// Runs a duplicate check that returns (hashes dropped, result) and, when it dropped any, a hashing
/// run to replace them.
async fn verified<T: Send + 'static>(
    app: &AppHandle,
    check: impl FnOnce(&Library) -> Result<(usize, T)> + Send + 'static,
) -> Result<(usize, T)> {
    let root = active_root(app)?;
    let path = root.clone();
    let result = background(move || check(&Library::writable(&path)?)).await?;
    if result.0 > 0 && active_root(app)? == root {
        start_backfill(app, root);
    }
    Ok(result)
}

/// Drops hashes of changed duplicate group members; returns how many.
#[tauri::command]
async fn verify_duplicates(app: AppHandle) -> Result<usize> {
    let (dropped, ()) = verified(&app, |library| Ok((library.verify_duplicates()?, ()))).await?;
    Ok(dropped)
}

#[derive(serde::Serialize)]
struct AssetDuplicates {
    copies: Vec<DuplicateCopy>,
    /// Hashes dropped by the check before reading; counts elsewhere changed when > 0.
    dropped: usize,
}

#[tauri::command]
async fn catalog_filters(app: AppHandle, package_id: Option<i64>) -> Result<CatalogFilters> {
    let root = active_root(&app)?;
    background(move || Library::writable(&root)?.catalog_filters(package_id)).await
}

#[tauri::command]
async fn create_folder(
    app: AppHandle,
    name: String,
    parent_id: Option<i64>,
) -> Result<ProjectFolder> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.create_folder(&name, parent_id)
    })
    .await
}

#[tauri::command]
async fn add_to_folder(app: AppHandle, folder_id: i64, asset_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.add_to_folder(folder_id, asset_id)
    })
    .await
}

#[tauri::command]
async fn remove_from_folder(app: AppHandle, folder_id: i64, asset_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.remove_from_folder(folder_id, asset_id)
    })
    .await
}

#[tauri::command]
async fn set_folder_member(
    app: AppHandle,
    folder_id: i64,
    asset_id: i64,
    file_id: Option<i64>,
    role: Option<String>,
) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.set_folder_member(folder_id, asset_id, file_id, role.as_deref())
    })
    .await
}

#[tauri::command]
async fn plan_project_export(app: AppHandle, folder_id: i64) -> Result<ProjectExportPlan> {
    let root = active_root(&app)?;
    background(move || Library::writable(&root)?.plan_project_export(folder_id)).await
}

#[tauri::command]
async fn export_project(app: AppHandle, folder_id: i64, parent: String) -> Result<ExportOutcome> {
    let parent = PathBuf::from(parent);
    selected_path(&app, &parent)?;
    let root = active_root(&app)?;
    // shortcut: one blocking copy without progress or cancel; add both if projects grow to many GiB.
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.export_project(folder_id, &parent)
    })
    .await
}

#[tauri::command]
async fn trash_package(app: AppHandle, package_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.trash_package(package_id)
    })
    .await
}

#[tauri::command]
async fn trash_file(app: AppHandle, file_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.trash_file(file_id)
    })
    .await
}

#[tauri::command]
async fn trash_entries(app: AppHandle) -> Result<Vec<TrashEntry>> {
    let root = active_root(&app)?;
    background(move || Library::writable(&root)?.trash_entries()).await
}

#[tauri::command]
async fn empty_trash(app: AppHandle) -> Result<EmptiedTrash> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.empty_trash()
    })
    .await
}

#[tauri::command]
async fn restore_trash(app: AppHandle, trash_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.restore_trash(trash_id)
    })
    .await
}

#[tauri::command]
async fn annotate_asset(
    app: AppHandle,
    asset_id: i64,
    name: String,
    tags: Vec<String>,
) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.annotate_asset(asset_id, &name, &tags)
    })
    .await
}

#[tauri::command]
async fn bulk_assets(
    app: AppHandle,
    selection: Vec<BulkSelection>,
    action: BulkAction,
) -> Result<BulkResult> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.bulk_assets(&selection, action)
    })
    .await
}

#[tauri::command]
async fn annotate_package(app: AppHandle, package_id: i64, tags: Vec<String>) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.annotate_package(package_id, &tags)
    })
    .await
}

#[tauri::command]
async fn saved_searches(app: AppHandle) -> Result<Vec<SavedSearch>> {
    let root = active_root(&app)?;
    background(move || Library::writable(&root)?.saved_searches()).await
}

#[tauri::command]
async fn save_search(app: AppHandle, name: String, query_json: String) -> Result<SavedSearch> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.save_search(&name, &query_json)
    })
    .await
}

#[tauri::command]
async fn delete_saved_search(app: AppHandle, id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.delete_saved_search(id)
    })
    .await
}

#[tauri::command]
async fn asset_duplicates(app: AppHandle, asset_id: i64) -> Result<AssetDuplicates> {
    let (dropped, copies) = verified(&app, move |library| {
        let dropped = library.verify_asset_duplicates(asset_id)?;
        Ok((dropped, library.asset_duplicates(asset_id)?))
    })
    .await?;
    Ok(AssetDuplicates { copies, dropped })
}

#[tauri::command]
async fn get_package_info(app: AppHandle, package_id: i64) -> Result<PackageInfoDetails> {
    let root = active_root(&app)?;
    background(move || Library::writable(&root)?.get_package_info(package_id)).await
}

#[tauri::command]
async fn set_package_info(app: AppHandle, package_id: i64, info: PackageInfo) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.set_package_info(package_id, &info)
    })
    .await
}

#[tauri::command]
async fn split_variant(app: AppHandle, file_id: i64) -> Result<i64> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.split_variant(file_id)
    })
    .await
}

#[tauri::command]
async fn assign_variant(app: AppHandle, file_id: i64, target_asset_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let _gate = catalog_write(&app, &root)?;
        Library::writable(&root)?.assign_variant(file_id, target_asset_id)
    })
    .await
}

#[tauri::command]
async fn resolve_preview(app: AppHandle, file_id: i64) -> Result<String> {
    let root = active_root(&app)?;
    background(move || {
        let file = Library::writable(&root)?.resolve_preview(file_id)?;
        Ok(file.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
async fn model_resources(app: AppHandle, file_id: i64) -> Result<ModelResources> {
    let (root, request) = app.state::<Session>().begin_preview()?;
    background(move || {
        let snapshot = Library::writable(&root)?.model_resources(file_id)?;
        pin_resources(&app, &root, request, &snapshot)?;
        Ok(snapshot)
    })
    .await
}

#[tauri::command]
async fn model_thumbnail_resources(app: AppHandle, file_id: i64) -> Result<ModelResources> {
    let root = active_root(&app)?;
    background(move || Library::open_read_only(&root)?.model_resources(file_id)).await
}

#[tauri::command]
async fn image_group(app: AppHandle, file_id: i64, workflow: ImageWorkflow) -> Result<ImageGroup> {
    let (root, request) = app.state::<Session>().begin_preview()?;
    background(move || {
        let group = Library::writable(&root)?.image_group(file_id, workflow)?;
        pin_resources(&app, &root, request, &group.resources)?;
        Ok(group)
    })
    .await
}

#[tauri::command]
async fn sheet_resources(app: AppHandle, file_id: i64) -> Result<ModelResources> {
    let (root, request) = app.state::<Session>().begin_preview()?;
    background(move || {
        let snapshot = Library::writable(&root)?.sheet_resources(file_id)?;
        pin_resources(&app, &root, request, &snapshot)?;
        Ok(snapshot)
    })
    .await
}

#[tauri::command]
async fn export_sprite(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<ExportOutcome> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("Sprite-Export benötigt binäre Bilddaten".into());
    };
    let plan = crate::library::SpritePayload::request_plan(bytes)?;
    selected_path(
        &app,
        plan.destination
            .parent()
            .ok_or("Exportziel hat keinen Ordner")?,
    )?;
    let state = app.state::<Session>();
    state.export_preview(&plan.snapshot_id, plan.file_id)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut job = state
            .sprite_export
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if job.is_some() {
            return Err("Bitte den laufenden Sprite-Export abwarten".into());
        }
        *job = Some((plan.snapshot_id.clone(), cancelled.clone()));
    }
    let bytes = bytes.clone();
    background(move || {
        let state = app.state::<Session>();
        let result = (|| {
            let payload = crate::library::SpritePayload::parse(&bytes)?;
            let _gate = file_gate(&state.import_gate)?;
            if cancelled.load(Ordering::SeqCst) {
                return Err("Sprite-Export wurde abgebrochen".into());
            }
            state.with_sprite_preview(
                &payload.plan.snapshot_id,
                payload.plan.file_id,
                |root, snapshot| {
                    Library::writable(root)?.export_sprite(snapshot, &payload, || {
                        if cancelled.load(Ordering::SeqCst) {
                            Err("Sprite-Export wurde abgebrochen".into())
                        } else {
                            Ok(())
                        }
                    })
                },
            )
        })();
        let mut job = state
            .sprite_export
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if job
            .as_ref()
            .is_some_and(|(_, flag)| Arc::ptr_eq(flag, &cancelled))
        {
            *job = None;
        }
        result
    })
    .await
}

#[tauri::command]
fn cancel_sprite_export(app: AppHandle, snapshot_id: String, dispose: bool) -> Result<()> {
    let state = app.state::<Session>();
    state.cancel_sprite_export(Some(&snapshot_id))?;
    if dispose {
        let mut preview = state
            .model_preview
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if preview
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.snapshot_id == snapshot_id)
        {
            preview.snapshot = None;
            preview.request = preview.request.wrapping_add(1);
        }
    }
    Ok(())
}

fn pin_resources(
    app: &AppHandle,
    root: &Path,
    request: u64,
    snapshot: &ModelResources,
) -> Result<()> {
    app.state::<Session>().pin_preview(root, request, snapshot)
}

#[tauri::command]
async fn export_model(
    app: AppHandle,
    snapshot_id: String,
    file_id: i64,
    resource_paths: Vec<String>,
    destination: Option<String>,
) -> Result<ExportOutcome> {
    let destination = destination.map(PathBuf::from);
    if let Some(destination) = &destination {
        selected_path(
            &app,
            destination.parent().ok_or("Exportziel hat keinen Ordner")?,
        )?;
    }
    background(move || {
        let state = app.state::<Session>();
        let _gate = file_gate(&state.import_gate)?;
        let root = active_root(&app)?;
        let snapshot = state.export_preview(&snapshot_id, file_id)?;
        Library::writable(&root)?.export_model(&snapshot, &resource_paths, destination.as_deref())
    })
    .await
}

#[tauri::command]
async fn prepare_audio_analysis(
    app: AppHandle,
    request_id: String,
    query: CatalogQuery,
    audio_criteria: Option<AudioCriteria>,
) -> Result<PreparedAudioAnalysis> {
    let (root, flag) = app.state::<Session>().begin_audio_analysis(&request_id)?;
    background(move || {
        let state = app.state::<Session>();
        let result = Library::open_read_only(&root)
            .and_then(|library| library.prepare_audio_analysis(query, audio_criteria));
        match result {
            Ok(files) => state.finish_audio_preparation(&root, &request_id, &flag, files),
            Err(error) => {
                state.finish_audio_analysis(&request_id, &flag)?;
                Err(error)
            }
        }
    })
    .await
}
#[tauri::command]
async fn run_audio_analysis(
    app: AppHandle,
    request_id: String,
    progress: tauri::ipc::Channel<AudioAnalysisProgress>,
) -> Result<AudioAnalysisProgress> {
    let (root, flag, files) = app.state::<Session>().take_audio_analysis(&request_id)?;
    background(move || {
        let state = app.state::<Session>();
        let mut last_report = None;
        let library = match Library::writable(&root) {
            Ok(library) => library,
            Err(error) => {
                state.finish_audio_analysis(&request_id, &flag)?;
                return Err(error);
            }
        };
        let summary =
            state.run_prepared_audio(&library, &root, &request_id, &flag, files, |value| {
                let now = Instant::now();
                if last_report
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(100))
                {
                    last_report = Some(now);
                    let _ = progress.send(value);
                }
            })?;
        let _ = progress.send(summary.clone());
        Ok(summary)
    })
    .await
}
#[tauri::command]
fn cancel_audio_analysis(app: AppHandle, request_id: String) -> Result<()> {
    app.state::<Session>()
        .cancel_audio_analysis(Some(&request_id))
}
#[tauri::command]
async fn analyze_audio(app: AppHandle, file_id: i64) -> Result<AudioMetadata> {
    let root = active_root(&app)?;
    background(move || {
        // No file gate: the store revalidates the source inside its own SQLite write transaction.
        let state = app.state::<Session>();
        let library = Library::writable(&root)?;
        // Short wait: the root lock below is held while SQLite waits for the writer.
        library.audio_analysis_busy_timeout()?;
        let flag = AtomicBool::new(false);
        let measured = library.inspect_audio(file_id, &flag)?;
        let current = state.root.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_deref() != Some(&root) {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        library.store_audio_inspection(file_id, measured, &flag)
    })
    .await
}

#[tauri::command]
async fn audio_tags(app: AppHandle, file_id: i64, request_id: String) -> Result<EmbeddedAudioTags> {
    let (root, cancelled) = app.state::<Session>().begin_audio_tags(&request_id)?;
    background(move || {
        let state = app.state::<Session>();
        let result = (|| {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Audio-Tag-Analyse abgebrochen".into());
            }
            let _gate = state
                .audio_tags_gate
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            Library::open_read_only(&root)?.embedded_audio_tags(file_id, &cancelled)
        })();
        state.finish_audio_tags(&root, &request_id, &cancelled)?;
        result
    })
    .await
}

#[tauri::command]
fn cancel_audio_tags(app: AppHandle, request_id: String) -> Result<()> {
    app.state::<Session>().cancel_audio_tags(Some(&request_id))
}

#[tauri::command]
async fn audio_waveform(app: AppHandle, file_id: i64, request_id: String) -> Result<AudioWaveform> {
    if request_id.is_empty() || request_id.len() > 128 {
        return Err("Ungültige Audioanalyse-Anfrage".into());
    }
    let (root, cancelled) = {
        let state = app.state::<Session>();
        let current_root = state.root.lock().unwrap_or_else(PoisonError::into_inner);
        let root = current_root
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut current = state
            .audio_waveform
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((_, previous)) = current.replace((request_id.clone(), cancelled.clone())) {
            previous.store(true, Ordering::Relaxed);
        }
        (root, cancelled)
    };
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .audio_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if cancelled.load(Ordering::Relaxed) {
            return Err("Wellenform-Analyse abgebrochen".into());
        }
        let result = Library::open_read_only(&root)
            .and_then(|library| library.audio_waveform(file_id, &cancelled));
        let root_matches = active_root(&app).is_ok_and(|current| current == root);
        let mut current = state
            .audio_waveform
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_some_and(|(id, _)| id == &request_id) {
            current.take();
        }
        if cancelled.load(Ordering::Relaxed) || !root_matches {
            return Err("Wellenform-Analyse abgebrochen".into());
        }
        result
    })
    .await
}

#[tauri::command]
fn cancel_audio_waveform(app: AppHandle, request_id: String) -> Result<()> {
    let state = app.state::<Session>();
    let mut current = state
        .audio_waveform
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if current.as_ref().is_some_and(|(id, _)| id == &request_id) {
        if let Some((_, cancelled)) = current.take() {
            cancelled.store(true, Ordering::Relaxed);
        }
    }
    Ok(())
}

#[tauri::command]
async fn export_asset(
    app: AppHandle,
    file_id: i64,
    destination: Option<String>,
    overwrite: bool,
) -> Result<ExportOutcome> {
    let destination = destination.map(PathBuf::from);
    if let Some(destination) = &destination {
        selected_path(&app, destination)?;
    }
    let root = active_root(&app)?;
    background(move || Library::writable(&root)?.export(file_id, destination.as_deref(), overwrite))
        .await
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Session::default())
        .setup(|app| {
            let remembered = app
                .path()
                .app_config_dir()
                .ok()
                .and_then(|dir| fs::read_to_string(dir.join(LAST_LIBRARY)).ok());
            // A moved or deleted library simply starts without one.
            if let Some(library) = remembered.and_then(|root| Library::open(Path::new(&root)).ok())
            {
                let _ = activate(app.handle(), &library);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            create_library,
            open_library,
            get_library,
            select_import_source,
            dropped_import_sources,
            import_folder,
            import_zip,
            estimate_zip,
            cancel_import,
            reference_folder,
            reference_zip,
            relink_source,
            query_catalog,
            catalog_filters,
            create_folder,
            add_to_folder,
            remove_from_folder,
            set_folder_member,
            plan_project_export,
            export_project,
            trash_package,
            trash_file,
            trash_entries,
            empty_trash,
            restore_trash,
            annotate_asset,
            bulk_assets,
            annotate_package,
            get_package_info,
            set_package_info,
            asset_duplicates,
            verify_duplicates,
            saved_searches,
            save_search,
            delete_saved_search,
            split_variant,
            assign_variant,
            resolve_preview,
            analyze_audio,
            prepare_audio_analysis,
            run_audio_analysis,
            cancel_audio_analysis,
            audio_tags,
            cancel_audio_tags,
            audio_waveform,
            cancel_audio_waveform,
            model_resources,
            model_thumbnail_resources,
            image_group,
            sheet_resources,
            export_sprite,
            cancel_sprite_export,
            export_model,
            export_asset
        ])
        .run(tauri::generate_context!())
        .expect("Lootbag could not start");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};

    #[test]
    fn a_panic_while_holding_the_file_gate_does_not_block_later_writes() {
        let gate = Mutex::new(());
        let _ = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _held = file_gate(&gate).unwrap();
                    panic!("simulated failure during a file operation");
                })
                .join()
        });
        assert!(gate.is_poisoned());
        let held = file_gate(&gate).unwrap();
        assert!(file_gate(&gate).is_err(), "a live holder still blocks");
        drop(held);
        assert!(file_gate(&gate).is_ok());
    }

    #[test]
    fn explicit_audio_population_jobs_keep_frozen_scope_partial_progress_and_cancel_owners() {
        let root = std::env::temp_dir().join(format!(
            "audio-job-session-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("source")).unwrap();
        for i in 0..45 {
            fs::write(
                root.join(format!("source/Tone{i:03}.mp3")),
                include_bytes!("../tests/fixtures/audio/stereo44100.mp3"),
            )
            .unwrap();
        }
        let mut library = Library::create(&root.join("Library")).unwrap();
        library.import_folder(&root.join("source")).unwrap();
        let session = Session::default();
        session.set_root(library.root().to_owned()).unwrap();
        let (first_root, older) = session.begin_audio_analysis("older-token").unwrap();
        let (_, latest) = session.begin_audio_analysis("latest-token").unwrap();
        assert!(
            session.begin_audio_analysis("latest-token").is_err(),
            "duplicate live request cannot create another cancellation owner"
        );
        assert!(older.load(Ordering::Relaxed));
        assert!(session
            .finish_audio_preparation(&first_root, "latest-token", &older, vec![1])
            .is_err());
        session.cancel_audio_analysis(Some("older-token")).unwrap();
        assert!(!latest.load(Ordering::Relaxed));
        let ids = library
            .prepare_audio_analysis(
                CatalogQuery {
                    kind: Some(crate::library::AssetKind::Sound),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let prepared = session
            .finish_audio_preparation(&first_root, "latest-token", &latest, ids)
            .unwrap();
        assert_eq!(prepared.total, 45);
        let (job_root, flag, frozen) = session.take_audio_analysis("latest-token").unwrap();
        assert!(
            session.take_audio_analysis("latest-token").is_err(),
            "prepared job is consumed once"
        );
        let mut progress = Vec::new();
        let summary = session
            .run_prepared_audio(
                &library,
                &job_root,
                "latest-token",
                &flag,
                frozen,
                |value| {
                    if value.processed == 1 {
                        session.cancel_audio_analysis(Some("latest-token")).unwrap();
                    }
                    progress.push(value);
                },
            )
            .unwrap();
        assert_eq!(summary.total, 45);
        assert_eq!(summary.processed, 1);
        assert_eq!(summary.ready, 1);
        assert_eq!(summary.errors, 0);
        assert_eq!(summary.skipped, 0);
        assert!(summary.cancelled);
        assert_eq!(progress.last().unwrap().processed, 1);
        drop(library);
        let library = Library::open(&job_root).unwrap();
        let page = library
            .query(CatalogQuery {
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            page.items
                .iter()
                .flat_map(|a| &a.files)
                .filter(|f| f.audio.is_some())
                .count(),
            1
        );
        let (next_root, next_flag) = session.begin_audio_analysis("finish-rest").unwrap();
        let remaining = library
            .prepare_audio_analysis(
                CatalogQuery {
                    kind: Some(crate::library::AssetKind::Sound),
                    ..Default::default()
                },
                Some(AudioCriteria {
                    analysis: Some("notAnalysed".into()),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert_eq!(remaining.len(), 44);
        session
            .finish_audio_preparation(&next_root, "finish-rest", &next_flag, remaining)
            .unwrap();
        let (next_root, next_flag, remaining) = session.take_audio_analysis("finish-rest").unwrap();
        let summary = session
            .run_prepared_audio(
                &library,
                &next_root,
                "finish-rest",
                &next_flag,
                remaining,
                |_| {},
            )
            .unwrap();
        assert_eq!(summary.processed, 44);
        assert_eq!(summary.ready, 44);
        assert!(!summary.cancelled);
        assert_eq!(
            library
                .query(CatalogQuery {
                    limit: 100,
                    ..Default::default()
                })
                .unwrap()
                .items
                .iter()
                .flat_map(|a| &a.files)
                .filter(|f| f.audio.is_some())
                .count(),
            45
        );
        let (_, replaced) = session.begin_audio_analysis("move-root").unwrap();
        session.set_root(root.join("another-library")).unwrap();
        assert!(replaced.load(Ordering::Relaxed));
        assert!(session
            .finish_audio_preparation(&job_root, "move-root", &replaced, vec![1])
            .is_err());
        assert!(session.begin_audio_analysis("").is_err());
        assert!(session.begin_audio_analysis(&"x".repeat(129)).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaced_audio_jobs_preserve_the_new_owner_and_report_errors_separately_from_unavailable_sources(
    ) {
        let root = std::env::temp_dir().join(format!(
            "audio-job-replace-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir(root.join("external")).unwrap();
        fs::write(
            root.join("source/Valid.mp3"),
            include_bytes!("../tests/fixtures/audio/stereo44100.mp3"),
        )
        .unwrap();
        fs::write(root.join("source/Broken.wav"), b"stable unreadable source").unwrap();
        fs::write(
            root.join("external/Offline.mp3"),
            include_bytes!("../tests/fixtures/audio/stereo44100.mp3"),
        )
        .unwrap();
        let mut library = Library::create(&root.join("Library")).unwrap();
        library.import_folder(&root.join("source")).unwrap();
        library.reference_folder(&root.join("external")).unwrap();
        let session = Session::default();
        session.set_root(library.root().to_owned()).unwrap();
        let (job_root, flag) = session.begin_audio_analysis("mixed").unwrap();
        let ids = library
            .prepare_audio_analysis(
                CatalogQuery {
                    kind: Some(crate::library::AssetKind::Sound),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        session
            .finish_audio_preparation(&job_root, "mixed", &flag, ids)
            .unwrap();
        fs::rename(root.join("external"), root.join("external-offline")).unwrap();
        let (job_root, flag, ids) = session.take_audio_analysis("mixed").unwrap();
        let summary = session
            .run_prepared_audio(&library, &job_root, "mixed", &flag, ids, |_| {})
            .unwrap();
        assert_eq!(
            (
                summary.total,
                summary.processed,
                summary.ready,
                summary.errors,
                summary.skipped,
                summary.cancelled
            ),
            (3, 3, 1, 1, 1, false)
        );
        let (job_root, flag) = session.begin_audio_analysis("replaced").unwrap();
        session
            .finish_audio_preparation(&job_root, "replaced", &flag, vec![1, 2])
            .unwrap();
        let (job_root, flag, ids) = session.take_audio_analysis("replaced").unwrap();
        let mut new_flag = None;
        let summary = session
            .run_prepared_audio(&library, &job_root, "replaced", &flag, ids, |value| {
                if value.processed == 1 && new_flag.is_none() {
                    let (_, new) = session.begin_audio_analysis("replacement").unwrap();
                    session
                        .finish_audio_preparation(&job_root, "replacement", &new, vec![1])
                        .unwrap();
                    new_flag = Some(new);
                }
            })
            .unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.processed, 1);
        let (_, replacement, ids) = session.take_audio_analysis("replacement").unwrap();
        assert!(Arc::ptr_eq(&replacement, &new_flag.unwrap()));
        assert_eq!(ids, vec![1]);
        session.cancel_audio_analysis(Some("replaced")).unwrap();
        assert!(!replacement.load(Ordering::Relaxed));
        let held = session.root.lock().unwrap();
        std::thread::scope(|scope| {
            scope
                .spawn(|| session.cancel_audio_analysis(Some("replacement")).unwrap())
                .join()
                .unwrap();
        });
        assert!(replacement.load(Ordering::Relaxed));
        drop(held);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn audio_tag_jobs_keep_cancel_tokens_independent_and_reject_stale_root_or_reversed_completion()
    {
        let session = Session::default();
        let first = PathBuf::from("/owned/first-library");
        session.set_root(first.clone()).unwrap();
        let waveform = Arc::new(AtomicBool::new(false));
        *session.audio_waveform.lock().unwrap() = Some(("waveform-job".into(), waveform.clone()));
        let (root, older) = session.begin_audio_tags("same-token").unwrap();
        let (_, latest) = session.begin_audio_tags("same-token").unwrap();
        assert!(older.load(Ordering::Relaxed));
        assert!(!latest.load(Ordering::Relaxed));
        assert!(!waveform.load(Ordering::Relaxed));
        assert!(session
            .finish_audio_tags(&root, "same-token", &older)
            .is_err());
        assert!(Arc::ptr_eq(
            &session.audio_tags.lock().unwrap().as_ref().unwrap().1,
            &latest
        ));
        session.cancel_audio_tags(Some("older-token")).unwrap();
        assert!(!latest.load(Ordering::Relaxed));
        session
            .finish_audio_tags(&root, "same-token", &latest)
            .unwrap();
        assert!(session.audio_tags.lock().unwrap().is_none());
        let (_, cancelled) = session.begin_audio_tags("cancel-this").unwrap();
        session.cancel_audio_tags(Some("cancel-this")).unwrap();
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(!waveform.load(Ordering::Relaxed));
        assert!(session
            .finish_audio_tags(&root, "cancel-this", &cancelled)
            .is_err());
        let (_, moved) = session.begin_audio_tags("switch-root").unwrap();
        session
            .set_root(PathBuf::from("/owned/next-library"))
            .unwrap();
        assert!(moved.load(Ordering::Relaxed));
        assert!(session
            .finish_audio_tags(&root, "switch-root", &moved)
            .is_err());
        assert!(session.begin_audio_tags("").is_err());
        assert!(session.begin_audio_tags(&"x".repeat(129)).is_err());
        assert!(session.audio_tags.lock().unwrap().is_none());
    }

    #[test]
    fn sprite_publication_uses_only_the_current_session_pin_and_cancellation_remains_live_under_publication_locks(
    ) {
        let root = std::env::temp_dir().join(format!(
            "sprite-session-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("sheet.png"), b"owned source stamp fixture").unwrap();
        let mut library = Library::create(&root.join("Library")).unwrap();
        library.import_folder(&source).unwrap();
        let id = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
        let snapshot = library.sheet_resources(id).unwrap();
        let session = Session::default();
        session.set_root(library.root().to_owned()).unwrap();
        let (current, request) = session.begin_preview().unwrap();
        session.pin_preview(&current, request, &snapshot).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        *session.sprite_export.lock().unwrap() =
            Some((snapshot.snapshot_id.clone(), cancelled.clone()));
        session
            .with_sprite_preview(&snapshot.snapshot_id, id, |root, owned| {
                assert_eq!(root, library.root());
                assert_eq!(owned.snapshot_id, snapshot.snapshot_id);
                session.cancel_sprite_export(Some("another-job"))?;
                assert!(!cancelled.load(Ordering::SeqCst));
                session.cancel_sprite_export(Some(&snapshot.snapshot_id))?;
                assert!(cancelled.load(Ordering::SeqCst));
                Ok(())
            })
            .unwrap();
        assert!(session
            .with_sprite_preview::<()>("old-snapshot", id, |_, _| panic!(
                "Stale source must not publish"
            ))
            .is_err());
        session.begin_preview().unwrap();
        assert!(session
            .with_sprite_preview::<()>(&snapshot.snapshot_id, id, |_, _| panic!(
                "Switched pin must not publish"
            ))
            .is_err());
        let (current, request) = session.begin_preview().unwrap();
        session.pin_preview(&current, request, &snapshot).unwrap();
        session.set_root(root.join("another-library")).unwrap();
        assert!(session
            .with_sprite_preview::<()>(&snapshot.snapshot_id, id, |_, _| panic!(
                "Switched root must not publish"
            ))
            .is_err());
        drop(library);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_the_latest_model_or_image_request_can_be_exported_after_reversed_completion() {
        let root = std::env::temp_dir().join(format!(
            "asset-preview-order-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        for name in [
            "Triangle.obj",
            "Stone_Base_Color.png",
            "water1.png",
            "water2.png",
        ] {
            fs::write(source.join(name), name).unwrap();
        }
        let mut library = Library::create(&root.join("library")).unwrap();
        library.import_folder(&source).unwrap();
        let file_id = |name: &str| {
            library
                .query(CatalogQuery {
                    text: name.into(),
                    ..Default::default()
                })
                .unwrap()
                .items[0]
                .files[0]
                .id
        };
        let model = library.model_resources(file_id("Triangle")).unwrap();
        let material = library
            .image_group(file_id("Stone"), ImageWorkflow::Material)
            .unwrap()
            .resources;
        let sequence = library
            .image_group(file_id("water1"), ImageWorkflow::SpriteSequence)
            .unwrap()
            .resources;
        let session = Session::default();
        session.set_root(library.root().to_owned()).unwrap();
        for (slow, current) in [
            (&model, &material),
            (&material, &sequence),
            (&sequence, &model),
        ] {
            let (active_root, slow_request) = session.begin_preview().unwrap();
            let (_, current_request) = session.begin_preview().unwrap();
            session
                .pin_preview(&active_root, current_request, current)
                .unwrap();
            assert!(session
                .pin_preview(&active_root, slow_request, slow)
                .is_err());
            assert_eq!(
                session
                    .export_preview(&current.snapshot_id, current.file_id)
                    .unwrap()
                    .snapshot_id,
                current.snapshot_id
            );
            assert!(session
                .export_preview(&slow.snapshot_id, slow.file_id)
                .is_err());
            let thumbnail = Library::open_read_only(library.root())
                .unwrap()
                .model_resources(model.file_id)
                .unwrap();
            assert_eq!(
                session
                    .export_preview(&current.snapshot_id, current.file_id)
                    .unwrap()
                    .snapshot_id,
                current.snapshot_id
            );
            assert!(session
                .export_preview(&thumbnail.snapshot_id, thumbnail.file_id)
                .is_err());
        }
        let (active_root, old_request) = session.begin_preview().unwrap();
        session.set_root(library.root().to_owned()).unwrap();
        assert!(session
            .pin_preview(&active_root, old_request, &model)
            .is_err());
        assert!(session
            .export_preview(&model.snapshot_id, model.file_id)
            .is_err());
        drop(library);
        fs::remove_dir_all(root).unwrap();
    }
}
