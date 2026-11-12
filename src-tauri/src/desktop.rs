use crate::library::{
    authorize_import_evidence, consume_import_evidence, prepare_duplicate_files,
    prepare_import_evidence, read_storage_report, run_duplicate_files, AssetPackage, AudioCriteria,
    AudioMetadata, AudioStatus, AudioWaveform, BulkAction, BulkResult, BulkSelection,
    CatalogFilters, CatalogPage, CatalogQuery, DuplicateProgress, DuplicateReport,
    EmbeddedAudioTags, Evidence, ExportOutcome, ImageGroup, ImageWorkflow, ImportControl,
    ImportProgress, Library, ModelResources, PreparationProgress, PreparedDuplicateCheck,
    PreparedPackage, ProjectFolder, Result, StorageProgress, StorageReport, TrashEntry,
    ZipEstimate,
};
use crate::library::{
    prepare_external_trash_removal, remove_external_trash, PreparedTrashRemoval, TrashRemovalEntry,
    TrashRemovalPlan,
};
use crate::library::{
    prepare_recovery, run_recovery, PreparedRecovery, RecoveryFault, RecoveryKind, RecoveryOutcome,
    RecoveryPreparation, RecoveryProgress, RecoveryRequest,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

#[cfg(all(test, target_os = "macos"))]
#[path = "trash_removal_tests.rs"]
mod trash_removal_tests;

#[derive(Default)]
struct Session {
    root: Mutex<Option<PathBuf>>,
    root_generation: AtomicU64,
    trash_removal: Mutex<Option<ReadyTrashRemoval>>,
    recovery: Mutex<Option<RecoveryJob>>,
    prepared_import: Mutex<Option<PreparedImportJob>>,
    preimport_gate: Mutex<()>,
    import_gate: Mutex<()>,
    import_control: Mutex<Option<Arc<ImportControl>>>,
    model_preview: Mutex<PreviewState>,
    audio_waveform: Mutex<Option<(String, Arc<AtomicBool>)>>,
    audio_gate: Mutex<()>,
    audio_tags: Mutex<Option<(String, Arc<AtomicBool>)>>,
    audio_tags_gate: Mutex<()>,
    sprite_export: Mutex<Option<(String, Arc<AtomicBool>)>>,
    storage_report: Mutex<Option<(String, Arc<AtomicBool>)>>,
    storage_gate: Mutex<()>,
    duplicates: Mutex<Option<DuplicateJob>>,
    duplicate_gate: Mutex<()>,
    audio_analysis: Mutex<Option<AudioAnalysisJob>>,
}

struct ReadyTrashRemoval {
    root: PathBuf,
    generation: u64,
    plan: TrashRemovalPlan,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportPhase {
    Preparing,
    Ready,
    Estimating,
    Importing,
}
struct PreparedImportJob {
    request_id: String,
    root: PathBuf,
    generation: u64,
    control: Arc<ImportControl>,
    plan: Option<PreparedPackage>,
    phase: ImportPhase,
    authorized: bool,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedImport {
    request_id: String,
    source: ImportSource,
    binding: &'static str,
    zip_estimate: Option<ZipEstimate>,
    evidence: Evidence,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizedImport {
    request_id: String,
    zip_estimate: Option<ZipEstimate>,
}
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum PreparedImportOutcome {
    Imported { package: AssetPackage },
    Cancelled,
}
struct DuplicateJob {
    request_id: String,
    root: PathBuf,
    cancelled: Arc<AtomicBool>,
    prepared: Option<PreparedDuplicateCheck>,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedDuplicates {
    request_id: String,
    total: usize,
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
    fn prepare_trash_removal(
        &self,
        root: &Path,
        entry: TrashRemovalEntry,
    ) -> Result<PreparedTrashRemoval> {
        let _gate = self
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        self.trash_removal
            .lock()
            .map_err(|_| "Papierkorb-Vorbereitung ist nicht verfügbar")?
            .take();
        if current.as_deref() != Some(root) {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        let generation = self.root_generation.load(Ordering::Relaxed);
        let plan = prepare_external_trash_removal(root, entry)?;
        let reply = plan.reply.clone();
        *self
            .trash_removal
            .lock()
            .map_err(|_| "Papierkorb-Vorbereitung ist nicht verfügbar")? =
            Some(ReadyTrashRemoval {
                root: root.into(),
                generation,
                plan,
            });
        Ok(reply)
    }

    fn remove_prepared_trash(&self, token: &str) -> Result<()> {
        let ready = {
            let mut ready = self
                .trash_removal
                .lock()
                .map_err(|_| "Papierkorb-Vorbereitung ist nicht verfügbar")?;
            if !ready.as_ref().is_some_and(|r| r.plan.reply.token == token) {
                return Err("Papierkorb-Vorbereitung wurde ersetzt oder verbraucht".into());
            }
            ready.take().ok_or("Papierkorb-Vorbereitung fehlt")?
        };
        let _gate = self
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != Some(&ready.root)
            || self.root_generation.load(Ordering::Relaxed) != ready.generation
        {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        self.cancel_storage(None)?;
        self.cancel_duplicates(None)?;
        self.cancel_prepared_import(None)?;
        self.cancel_recovery(None)?;
        remove_external_trash(&ready.root, ready.plan)
    }

    fn discard_trash_removal(&self, token: &str) -> Result<()> {
        let mut ready = self
            .trash_removal
            .lock()
            .map_err(|_| "Papierkorb-Vorbereitung ist nicht verfügbar")?;
        if ready.as_ref().is_some_and(|r| r.plan.reply.token == token) {
            ready.take();
        }
        Ok(())
    }

    fn begin_preimport(
        &self,
        root: &Path,
        request_id: &str,
    ) -> Result<(PathBuf, u64, Arc<ImportControl>)> {
        if request_id.is_empty()
            || request_id.len() > 128
            || root.as_os_str().as_encoded_bytes().len() > 4096
        {
            return Err("Ungültige Importvorbereitung-Anfrage".into());
        }
        let _gate = self
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != Some(root) {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        let mut jobs = self
            .prepared_import
            .lock()
            .map_err(|_| "Importvorbereitung ist nicht verfügbar")?;
        if jobs.as_ref().is_some_and(|j| j.request_id == request_id) {
            return Err("Diese Import-Anfrage ist bereits registriert".into());
        }
        let control = Arc::new(ImportControl::default());
        let generation = self.root_generation.load(Ordering::Relaxed);
        if let Some(old) = jobs.replace(PreparedImportJob {
            request_id: request_id.into(),
            root: root.into(),
            generation,
            control: control.clone(),
            plan: None,
            phase: ImportPhase::Preparing,
            authorized: false,
        }) {
            old.control.cancel();
        }
        Ok((root.into(), generation, control))
    }
    fn attach_preimport(
        &self,
        root: &Path,
        generation: u64,
        request_id: &str,
        control: &Arc<ImportControl>,
        plan: PreparedPackage,
        authorized: bool,
    ) -> Result<PreparedImport> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut jobs = self
            .prepared_import
            .lock()
            .map_err(|_| "Importvorbereitung ist nicht verfügbar")?;
        let job = jobs
            .as_mut()
            .filter(|j| {
                j.request_id == request_id
                    && Arc::ptr_eq(&j.control, control)
                    && j.generation == generation
            })
            .ok_or("Import-Anfrage wurde ersetzt")?;
        if current.as_deref() != Some(root)
            || self.root_generation.load(Ordering::Relaxed) != generation
            || control.cancellation_flag().load(Ordering::Relaxed)
            || !matches!(job.phase, ImportPhase::Preparing | ImportPhase::Estimating)
        {
            return Err("Importvorbereitung ist nicht mehr aktuell".into());
        }
        let reply = PreparedImport {
            request_id: request_id.into(),
            source: ImportSource {
                path: plan
                    .source
                    .to_str()
                    .ok_or("Importpfad ist kein UTF-8")?
                    .into(),
                is_zip: plan.zip,
            },
            binding: if plan.verified() {
                "verified"
            } else {
                "unverified"
            },
            zip_estimate: None,
            evidence: plan.evidence.clone(),
        };
        job.plan = Some(plan);
        job.authorized = authorized;
        job.phase = ImportPhase::Ready;
        Ok(reply)
    }
    fn take_preimport(
        &self,
        request_id: &str,
        estimate: bool,
    ) -> Result<(PathBuf, u64, Arc<ImportControl>, PreparedPackage, bool)> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut jobs = self
            .prepared_import
            .lock()
            .map_err(|_| "Importvorbereitung ist nicht verfügbar")?;
        let job = jobs
            .as_mut()
            .filter(|j| {
                j.request_id == request_id
                    && j.phase == ImportPhase::Ready
                    && current.as_deref() == Some(j.root.as_path())
                    && self.root_generation.load(Ordering::Relaxed) == j.generation
                    && !j.control.cancellation_flag().load(Ordering::Relaxed)
            })
            .ok_or("Bitte den Import erneut vorbereiten")?;
        if estimate && job.plan.as_ref().is_some_and(|p| p.verified()) {
            return Err("Der Ordnerplan ist bereits geprüft".into());
        }
        if !estimate && !job.authorized && job.plan.as_ref().is_some_and(|p| !p.verified()) {
            return Err("Bitte den gesamten ungeprüften Import ausdrücklich bestätigen".into());
        }
        let plan = job
            .plan
            .take()
            .ok_or("Importplan wurde bereits verbraucht")?;
        job.phase = if estimate {
            ImportPhase::Estimating
        } else {
            ImportPhase::Importing
        };
        Ok((
            job.root.clone(),
            job.generation,
            job.control.clone(),
            plan,
            job.authorized,
        ))
    }
    fn with_preimport_root(
        &self,
        root: &Path,
        generation: u64,
        request_id: &str,
        control: &Arc<ImportControl>,
        operation: &mut dyn FnMut() -> Result<AssetPackage>,
    ) -> Result<AssetPackage> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != Some(root)
            || self.root_generation.load(Ordering::Relaxed) != generation
            || control.cancellation_flag().load(Ordering::Relaxed)
        {
            return Err("Import ist nicht mehr aktuell".into());
        }
        if !self
            .prepared_import
            .lock()
            .map_err(|_| "Importvorbereitung ist nicht verfügbar")?
            .as_ref()
            .is_some_and(|j| {
                j.request_id == request_id
                    && Arc::ptr_eq(&j.control, control)
                    && j.phase == ImportPhase::Importing
            })
        {
            return Err("Import-Anfrage wurde ersetzt".into());
        }
        operation()
    }
    fn finish_preimport(&self, request_id: &str, control: &Arc<ImportControl>) -> Result<()> {
        let mut jobs = self
            .prepared_import
            .lock()
            .map_err(|_| "Importvorbereitung ist nicht verfügbar")?;
        if jobs
            .as_ref()
            .is_some_and(|j| j.request_id == request_id && Arc::ptr_eq(&j.control, control))
        {
            jobs.take();
        }
        Ok(())
    }
    fn cancel_prepared_import(&self, request_id: Option<&str>) -> Result<()> {
        let mut jobs = self
            .prepared_import
            .lock()
            .map_err(|_| "Importvorbereitung ist nicht verfügbar")?;
        if let Some(job) = jobs
            .as_mut()
            .filter(|j| request_id.is_none_or(|id| j.request_id == id))
        {
            job.control.cancel();
            job.plan.take();
        }
        Ok(())
    }

    // Register before background preparation, and retain a cancelled unfinished owner.
    fn begin_duplicates(
        &self,
        root: &Path,
        request_id: &str,
    ) -> Result<(PathBuf, Arc<AtomicBool>)> {
        if request_id.is_empty()
            || request_id.len() > 128
            || root.as_os_str().as_encoded_bytes().len() > 4096
        {
            return Err("Ungültige Duplikatprüfung-Anfrage".into());
        }
        let _gate = self
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != Some(root) {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        let mut jobs = self
            .duplicates
            .lock()
            .map_err(|_| "Duplikatprüfung ist nicht verfügbar")?;
        if jobs.as_ref().is_some_and(|j| j.request_id == request_id) {
            return Err("Diese Duplikatprüfung-Anfrage ist bereits registriert".into());
        }
        let flag = Arc::new(AtomicBool::new(false));
        if let Some(old) = jobs.replace(DuplicateJob {
            request_id: request_id.into(),
            root: root.into(),
            cancelled: flag.clone(),
            prepared: None,
        }) {
            old.cancelled.store(true, Ordering::Relaxed);
        }
        Ok((root.into(), flag))
    }
    fn finish_duplicate_preparation(
        &self,
        root: &Path,
        request_id: &str,
        flag: &Arc<AtomicBool>,
        prepared: PreparedDuplicateCheck,
    ) -> Result<PreparedDuplicates> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut jobs = self
            .duplicates
            .lock()
            .map_err(|_| "Duplikatprüfung ist nicht verfügbar")?;
        let job = jobs
            .as_mut()
            .filter(|j| j.request_id == request_id && Arc::ptr_eq(&j.cancelled, flag))
            .ok_or("Duplikatprüfung-Anfrage wurde ersetzt")?;
        if current.as_deref() != Some(root) || flag.load(Ordering::Relaxed) {
            return Err("Duplikatprüfung abgebrochen".into());
        }
        let total = prepared.total();
        job.prepared = Some(prepared);
        Ok(PreparedDuplicates {
            request_id: request_id.into(),
            total,
        })
    }
    // Consume the prepared snapshot before spawning the scanner; Start cannot run it twice.
    fn take_duplicates(
        &self,
        request_id: &str,
    ) -> Result<(PathBuf, Arc<AtomicBool>, PreparedDuplicateCheck)> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut jobs = self
            .duplicates
            .lock()
            .map_err(|_| "Duplikatprüfung ist nicht verfügbar")?;
        let job = jobs
            .as_mut()
            .filter(|j| {
                j.request_id == request_id
                    && !j.cancelled.load(Ordering::Relaxed)
                    && current.as_deref() == Some(j.root.as_path())
            })
            .ok_or("Bitte die Duplikatprüfung erneut vorbereiten")?;
        let prepared = job
            .prepared
            .take()
            .ok_or("Duplikatprüfung ist nicht vorbereitet oder läuft bereits")?;
        Ok((job.root.clone(), job.cancelled.clone(), prepared))
    }
    fn finish_duplicates(
        &self,
        root: &Path,
        request_id: &str,
        flag: &Arc<AtomicBool>,
    ) -> Result<bool> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut jobs = self
            .duplicates
            .lock()
            .map_err(|_| "Duplikatprüfung ist nicht verfügbar")?;
        let matches = jobs
            .as_ref()
            .is_some_and(|j| j.request_id == request_id && Arc::ptr_eq(&j.cancelled, flag));
        if matches {
            jobs.take();
        }
        if !matches || current.as_deref() != Some(root) {
            return Err("Duplikatprüfung ist nicht mehr aktuell".into());
        }
        Ok(flag.load(Ordering::Relaxed))
    }
    fn cancel_duplicates(&self, request_id: Option<&str>) -> Result<()> {
        let mut jobs = self
            .duplicates
            .lock()
            .map_err(|_| "Duplikatprüfung ist nicht verfügbar")?;
        if let Some(job) = jobs
            .as_mut()
            .filter(|j| request_id.is_none_or(|id| j.request_id == id))
        {
            job.cancelled.store(true, Ordering::Relaxed);
            job.prepared.take(); // ACK also releases any prepared snapshot held while awaiting Start.
        }
        Ok(())
    }
    fn begin_storage(&self, root: &Path, request_id: &str) -> Result<(PathBuf, Arc<AtomicBool>)> {
        if request_id.is_empty()
            || request_id.len() > 128
            || root.as_os_str().as_encoded_bytes().len() > 4096
        {
            return Err("Ungültige Speicherbericht-Anfrage".into());
        }
        let _gate = self
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != Some(root) {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut job = self
            .storage_report
            .lock()
            .map_err(|_| "Speichermessung ist nicht verfügbar")?;
        if job.as_ref().is_some_and(|(id, _)| id == request_id) {
            return Err("Diese Speicherbericht-Anfrage läuft bereits".into());
        }
        if let Some((_, older)) = job.replace((request_id.into(), cancelled.clone())) {
            older.store(true, Ordering::Relaxed);
        }
        Ok((root.to_owned(), cancelled))
    }
    fn finish_storage(
        &self,
        root: &Path,
        request_id: &str,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<bool> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut job = self
            .storage_report
            .lock()
            .map_err(|_| "Speichermessung ist nicht verfügbar")?;
        let matches = job
            .as_ref()
            .is_some_and(|(id, flag)| id == request_id && Arc::ptr_eq(flag, cancelled));
        if matches {
            job.take();
        }
        if !matches || current.as_deref() != Some(root) {
            return Err("Speicherbericht ist nicht mehr aktuell".into());
        }
        Ok(cancelled.load(Ordering::Relaxed))
    }
    fn cancel_storage(&self, request_id: Option<&str>) -> Result<()> {
        let job = self
            .storage_report
            .lock()
            .map_err(|_| "Speichermessung ist nicht verfügbar")?;
        if let Some((id, flag)) = job.as_ref() {
            if request_id.is_none_or(|requested| requested == id) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    fn begin_audio_analysis(&self, request_id: &str) -> Result<(PathBuf, Arc<AtomicBool>)> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err("Ungültige Audioanalyse-Anfrage".into());
        }
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let root = current
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let flag = Arc::new(AtomicBool::new(false));
        let mut job = self
            .audio_analysis
            .lock()
            .map_err(|_| "Audioanalyse nicht verfügbar")?;
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
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut jobs = self
            .audio_analysis
            .lock()
            .map_err(|_| "Audioanalyse nicht verfügbar")?;
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
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let root = current
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let mut jobs = self
            .audio_analysis
            .lock()
            .map_err(|_| "Audioanalyse nicht verfügbar")?;
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
            .map_err(|_| "Audioanalyse nicht verfügbar")?;
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
            .map_err(|_| "Audioanalyse nicht verfügbar")?;
        Ok(!flag.load(Ordering::Relaxed)
            && jobs.as_ref().is_some_and(|job| {
                job.request_id == request_id && Arc::ptr_eq(&job.cancelled, flag)
            }))
    }
    fn finish_audio_analysis(&self, request_id: &str, flag: &Arc<AtomicBool>) -> Result<()> {
        let mut jobs = self
            .audio_analysis
            .lock()
            .map_err(|_| "Audioanalyse nicht verfügbar")?;
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
            let _gate = self
                .import_gate
                .try_lock()
                .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
            self.cancel_storage(None)?;
            self.cancel_duplicates(None)?;
            self.cancel_prepared_import(None)?;
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
                        let current = self
                            .root
                            .lock()
                            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
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
                    .map_err(|_| "Bibliothek ist nicht verfügbar")?
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
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let root = current
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut job = self
            .audio_tags
            .lock()
            .map_err(|_| "Audio-Tag-Analyse ist nicht verfügbar")?;
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
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut job = self
            .audio_tags
            .lock()
            .map_err(|_| "Audio-Tag-Analyse ist nicht verfügbar")?;
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
            .map_err(|_| "Audio-Tag-Analyse ist nicht verfügbar")?;
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
            .map_err(|_| "Sprite-Export ist nicht verfügbar")?;
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
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let preview = self
            .model_preview
            .lock()
            .map_err(|_| "Vorschau ist nicht verfügbar")?;
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
        self.cancel_storage(None)?;
        let mut current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut preview = self
            .model_preview
            .lock()
            .map_err(|_| "Vorschau ist nicht verfügbar")?;
        self.cancel_duplicates(None)?;
        self.cancel_prepared_import(None)?;
        self.cancel_recovery(None)?;
        self.trash_removal
            .lock()
            .map_err(|_| "Papierkorb-Vorbereitung ist nicht verfügbar")?
            .take();
        self.root_generation.fetch_add(1, Ordering::Relaxed);
        *current = Some(root);
        preview.request = preview.request.wrapping_add(1);
        preview.snapshot = None;
        if let Some((_, cancelled)) = self
            .audio_waveform
            .lock()
            .map_err(|_| "Audioanalyse ist nicht verfügbar")?
            .take()
        {
            cancelled.store(true, Ordering::Relaxed);
        }
        self.cancel_audio_tags(None)?;
        self.cancel_audio_analysis(None)?;
        Ok(())
    }

    fn begin_preview(&self) -> Result<(PathBuf, u64)> {
        self.cancel_sprite_export(None)?;
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let root = current
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let mut preview = self
            .model_preview
            .lock()
            .map_err(|_| "Vorschau ist nicht verfügbar")?;
        preview.request = preview.request.wrapping_add(1);
        preview.snapshot = None;
        Ok((root, preview.request))
    }

    fn allow_thumbnail(
        &self,
        root: &Path,
        allow_resources: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != Some(root) {
            return Err("Die Bibliothek wurde während der Vorschau-Vorbereitung gewechselt".into());
        }
        allow_resources()
    }

    fn pin_preview(
        &self,
        root: &Path,
        request: u64,
        snapshot: &ModelResources,
        allow_resources: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != Some(root) {
            return Err("Die Bibliothek wurde während der Vorschau-Vorbereitung gewechselt".into());
        }
        let mut preview = self
            .model_preview
            .lock()
            .map_err(|_| "Vorschau ist nicht verfügbar")?;
        if preview.request != request {
            return Err("Die Vorschau wurde während der Vorbereitung gewechselt".into());
        }
        allow_resources()?;
        preview.snapshot = Some(snapshot.clone());
        Ok(())
    }

    fn export_preview(&self, snapshot_id: &str, file_id: i64) -> Result<ModelResources> {
        let snapshot = self
            .model_preview
            .lock()
            .map_err(|_| "Vorschau ist nicht verfügbar")?
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
        packages,
        folders: library.folders()?,
    })
}

fn active_root(app: &AppHandle) -> Result<PathBuf> {
    app.state::<Session>()
        .root
        .lock()
        .map_err(|_| "Bibliothek ist nicht verfügbar")?
        .clone()
        .ok_or_else(|| "Bitte zuerst eine Bibliothek öffnen".into())
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
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        let library = match chosen {
            Some(root) => Library::create(&root)?,
            None => Library::create_default(&home)?,
        };
        let result = info(&library)?;
        state.set_root(library.root().to_owned())?;
        Ok(result)
    })
    .await
}

#[tauri::command]
async fn open_library(app: AppHandle, root: String) -> Result<LibraryInfo> {
    let root = PathBuf::from(root);
    selected_path(&app, &root)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        let library = Library::open(&root)?;
        let result = info(&library)?;
        state.set_root(library.root().to_owned())?;
        Ok(result)
    })
    .await
}

#[tauri::command]
async fn get_library(app: AppHandle) -> Result<Option<LibraryInfo>> {
    let root = app
        .state::<Session>()
        .root
        .lock()
        .map_err(|_| "Bibliothek ist nicht verfügbar")?
        .clone();
    background(move || {
        root.map(|root| Library::open(&root).and_then(|library| info(&library)))
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
    background(move || Library::open(&root)?.estimate_zip(&source)).await
}

#[tauri::command]
fn cancel_import(app: AppHandle) -> Result<bool> {
    let state = app.state::<Session>();
    let control = state
        .import_control
        .lock()
        .map_err(|_| "Importstatus nicht verfügbar")?;
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
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Es läuft bereits ein Import")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        let control = Arc::new(ImportControl::default());
        *state
            .import_control
            .lock()
            .map_err(|_| "Importstatus nicht verfügbar")? = Some(control.clone());
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
        let result = Library::open(&root).and_then(|mut library| {
            if is_zip {
                library.import_zip_with_progress(&source, &control, progress)
            } else {
                library.import_folder_with_progress(&source, &control, progress)
            }
        });
        *state
            .import_control
            .lock()
            .map_err(|_| "Importstatus nicht verfügbar")? = None;
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
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.reference_folder(&source)
    })
    .await
}

#[tauri::command]
async fn relink_source(app: AppHandle, package_id: i64, source: String) -> Result<AssetPackage> {
    let source = PathBuf::from(source);
    selected_path(&app, &source)?;
    let root = active_root(&app)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.relink_source(package_id, &source)
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
    background(move || Library::open(&root)?.query_with_audio(query, audio_criteria)).await
}

#[tauri::command]
async fn catalog_filters(app: AppHandle, package_id: Option<i64>) -> Result<CatalogFilters> {
    let root = active_root(&app)?;
    background(move || Library::open(&root)?.catalog_filters(package_id)).await
}

#[tauri::command]
async fn create_folder(
    app: AppHandle,
    name: String,
    parent_id: Option<i64>,
) -> Result<ProjectFolder> {
    let root = active_root(&app)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.create_folder(&name, parent_id)
    })
    .await
}

#[tauri::command]
async fn add_to_folder(app: AppHandle, folder_id: i64, asset_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.add_to_folder(folder_id, asset_id)
    })
    .await
}

#[tauri::command]
async fn remove_from_folder(app: AppHandle, folder_id: i64, asset_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.remove_from_folder(folder_id, asset_id)
    })
    .await
}

#[tauri::command]
async fn trash_package(app: AppHandle, package_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        Library::open(&root)?.trash_package(package_id)
    })
    .await
}

#[tauri::command]
async fn trash_file(app: AppHandle, file_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        Library::open(&root)?.trash_file(file_id)
    })
    .await
}

#[tauri::command]
async fn trash_entries(app: AppHandle) -> Result<Vec<TrashEntry>> {
    let root = active_root(&app)?;
    background(move || Library::open(&root)?.trash_entries()).await
}

#[tauri::command]
async fn prepare_trash_removal(
    app: AppHandle,
    root: String,
    entry: TrashRemovalEntry,
) -> Result<PreparedTrashRemoval> {
    background(move || {
        app.state::<Session>()
            .prepare_trash_removal(Path::new(&root), entry)
    })
    .await
}

#[tauri::command]
async fn remove_prepared_trash(app: AppHandle, token: String) -> Result<()> {
    background(move || app.state::<Session>().remove_prepared_trash(&token)).await
}

#[tauri::command]
fn discard_trash_removal(app: AppHandle, token: String) -> Result<()> {
    app.state::<Session>().discard_trash_removal(&token)
}

#[tauri::command]
async fn restore_trash(app: AppHandle, trash_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.restore_trash(trash_id)
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
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.annotate_asset(asset_id, &name, &tags)
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
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.bulk_assets(&selection, action)
    })
    .await
}

#[tauri::command]
async fn annotate_package(app: AppHandle, package_id: i64, tags: Vec<String>) -> Result<()> {
    let root = active_root(&app)?;
    background(move || {
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_prepared_import(None)?;
        if active_root(&app)? != root {
            return Err("Die aktive Bibliothek wurde geändert".into());
        }
        Library::open(&root)?.annotate_package(package_id, &tags)
    })
    .await
}

#[tauri::command]
async fn split_variant(app: AppHandle, file_id: i64) -> Result<i64> {
    let root = active_root(&app)?;
    background(move || Library::open(&root)?.split_variant(file_id)).await
}

#[tauri::command]
async fn assign_variant(app: AppHandle, file_id: i64, target_asset_id: i64) -> Result<()> {
    let root = active_root(&app)?;
    background(move || Library::open(&root)?.assign_variant(file_id, target_asset_id)).await
}

#[tauri::command]
async fn resolve_preview(app: AppHandle, file_id: i64) -> Result<String> {
    let root = active_root(&app)?;
    background(move || {
        let file = Library::open(&root)?.resolve_preview(file_id)?;
        app.asset_protocol_scope()
            .allow_file(&file)
            .map_err(|error| format!("Vorschau kann nicht freigegeben werden: {error}"))?;
        Ok(file.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
async fn model_resources(app: AppHandle, file_id: i64) -> Result<ModelResources> {
    let (root, request) = app.state::<Session>().begin_preview()?;
    background(move || {
        let snapshot = Library::open(&root)?.model_resources(file_id)?;
        pin_resources(&app, &root, request, &snapshot)?;
        Ok(snapshot)
    })
    .await
}

#[tauri::command]
async fn model_thumbnail_resources(app: AppHandle, file_id: i64) -> Result<ModelResources> {
    let root = active_root(&app)?;
    background(move || {
        let snapshot = Library::open_read_only(&root)?.model_resources(file_id)?;
        app.state::<Session>()
            .allow_thumbnail(&root, || allow_resource_files(&app, &snapshot))?;
        Ok(snapshot)
    })
    .await
}

#[tauri::command]
async fn image_group(app: AppHandle, file_id: i64, workflow: ImageWorkflow) -> Result<ImageGroup> {
    let (root, request) = app.state::<Session>().begin_preview()?;
    background(move || {
        let group = Library::open(&root)?.image_group(file_id, workflow)?;
        pin_resources(&app, &root, request, &group.resources)?;
        Ok(group)
    })
    .await
}

#[tauri::command]
async fn sheet_resources(app: AppHandle, file_id: i64) -> Result<ModelResources> {
    let (root, request) = app.state::<Session>().begin_preview()?;
    background(move || {
        let snapshot = Library::open(&root)?.sheet_resources(file_id)?;
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
            .map_err(|_| "Sprite-Export ist nicht verfügbar")?;
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
            let _gate = state
                .import_gate
                .try_lock()
                .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
            if cancelled.load(Ordering::SeqCst) {
                return Err("Sprite-Export wurde abgebrochen".into());
            }
            state.with_sprite_preview(
                &payload.plan.snapshot_id,
                payload.plan.file_id,
                |root, snapshot| {
                    Library::open(root)?.export_sprite(snapshot, &payload, || {
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
            .map_err(|_| "Sprite-Export ist nicht verfügbar")?;
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
            .map_err(|_| "Vorschau ist nicht verfügbar")?;
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
    app.state::<Session>()
        .pin_preview(root, request, snapshot, || {
            allow_resource_files(app, snapshot)
        })
}

fn allow_resource_files(app: &AppHandle, snapshot: &ModelResources) -> Result<()> {
    for resource in &snapshot.resources {
        app.asset_protocol_scope()
            .allow_file(&resource.path)
            .map_err(|error| format!("Asset-Ressource kann nicht freigegeben werden: {error}"))?;
    }
    Ok(())
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
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        let root = active_root(&app)?;
        let snapshot = state.export_preview(&snapshot_id, file_id)?;
        Library::open(&root)?.export_model(&snapshot, &resource_paths, destination.as_deref())
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
        let library = match Library::open(&root) {
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
        let state = app.state::<Session>();
        let _gate = state
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        state.cancel_storage(None)?;
        state.cancel_duplicates(None)?;
        state.cancel_prepared_import(None)?;
        let library = Library::open(&root)?;
        library.audio_analysis_busy_timeout()?;
        let flag = AtomicBool::new(false);
        let measured = library.inspect_audio(file_id, &flag)?;
        let current = state
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
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
                .map_err(|_| "Audio-Tag-Analyse ist nicht verfügbar")?;
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
        let current_root = state
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let root = current_root
            .clone()
            .ok_or("Bitte zuerst eine Bibliothek öffnen")?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut current = state
            .audio_waveform
            .lock()
            .map_err(|_| "Audioanalyse ist nicht verfügbar")?;
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
            .map_err(|_| "Audioanalyse ist nicht verfügbar")?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Wellenform-Analyse abgebrochen".into());
        }
        let result = Library::open_read_only(&root)
            .and_then(|library| library.audio_waveform(file_id, &cancelled));
        let root_matches = active_root(&app).is_ok_and(|current| current == root);
        let mut current = state
            .audio_waveform
            .lock()
            .map_err(|_| "Audioanalyse ist nicht verfügbar")?;
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
        .map_err(|_| "Audioanalyse ist nicht verfügbar")?;
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
    background(move || Library::open(&root)?.export(file_id, destination.as_deref(), overwrite))
        .await
}

#[tauri::command]
async fn prepare_import_package(
    app: AppHandle,
    root: String,
    request_id: String,
    source: String,
    on_progress: tauri::ipc::Channel<PreparationProgress>,
) -> Result<PreparedImport> {
    let source = PathBuf::from(source);
    selected_path(&app, &source)?;
    let (root, generation, control) = app
        .state::<Session>()
        .begin_preimport(Path::new(&root), &request_id)?;
    background(move || {
        let state = app.state::<Session>();
        let result = (|| {
            let _gate = state
                .preimport_gate
                .try_lock()
                .map_err(|_| "Eine frühere Importprüfung läuft noch; bitte kurz abwarten")?;
            let descriptor =
                describe_import_source(source.to_str().ok_or("Importpfad ist kein UTF-8")?.into())?;
            let plan = prepare_import_evidence(&root, &source, descriptor.is_zip, &control, |p| {
                let _ = on_progress.send(p);
            })?;
            state.attach_preimport(&root, generation, &request_id, &control, plan, false)
        })();
        if result.is_err() {
            let _ = state.finish_preimport(&request_id, &control);
        }
        result
    })
    .await
}
#[tauri::command]
async fn authorize_prepared_import(app: AppHandle, request_id: String) -> Result<AuthorizedImport> {
    let (root, generation, control, plan, _) =
        app.state::<Session>().take_preimport(&request_id, true)?;
    background(move || {
        let state = app.state::<Session>();
        let result = (|| {
            let _gate = state
                .preimport_gate
                .try_lock()
                .map_err(|_| "Bitte die laufende Importprüfung abwarten")?;
            let zip_estimate = authorize_import_evidence(&plan, &root, &control)?;
            state.attach_preimport(&root, generation, &request_id, &control, plan, true)?;
            Ok(AuthorizedImport {
                request_id: request_id.clone(),
                zip_estimate,
            })
        })();
        if result.is_err() {
            let _ = state.finish_preimport(&request_id, &control);
        }
        result
    })
    .await
}
#[tauri::command]
async fn import_prepared_package(
    app: AppHandle,
    request_id: String,
    on_progress: tauri::ipc::Channel<ImportProgress>,
) -> Result<PreparedImportOutcome> {
    let (root, generation, control, plan, authorized) =
        app.state::<Session>().take_preimport(&request_id, false)?;
    background(move || {
        let state = app.state::<Session>();
        let result = (|| {
            let _gate = state
                .import_gate
                .try_lock()
                .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
            state.cancel_storage(None)?;
            state.cancel_duplicates(None)?;
            let mut progress = |p| {
                let _ = on_progress.send(p);
            };
            let mut publisher = |operation: &mut dyn FnMut() -> Result<AssetPackage>| {
                state.with_preimport_root(&root, generation, &request_id, &control, operation)
            };
            consume_import_evidence(
                &root,
                plan,
                authorized,
                &control,
                &mut progress,
                &mut publisher,
            )
        })();
        state.finish_preimport(&request_id, &control)?;
        match result {
            Ok(package) => Ok(PreparedImportOutcome::Imported { package }),
            Err(_)
                if control.cancellation_flag().load(Ordering::Relaxed)
                    && !control.cleanup_failed() =>
            {
                Ok(PreparedImportOutcome::Cancelled)
            }
            Err(e) => Err(e),
        }
    })
    .await
}
#[tauri::command]
fn cancel_prepared_import(app: AppHandle, request_id: String) -> Result<()> {
    app.state::<Session>()
        .cancel_prepared_import(Some(&request_id))
}

#[tauri::command]
async fn prepare_duplicate_check(
    app: AppHandle,
    root: String,
    request_id: String,
    package_id: Option<i64>,
) -> Result<PreparedDuplicates> {
    let (root, flag) = app
        .state::<Session>()
        .begin_duplicates(Path::new(&root), &request_id)?;
    background(move || {
        let state = app.state::<Session>();
        let result = prepare_duplicate_files(&root, package_id, &flag).and_then(|prepared| {
            state.finish_duplicate_preparation(&root, &request_id, &flag, prepared)
        });
        if result.is_err() {
            let _ = state.finish_duplicates(&root, &request_id, &flag);
        }
        result
    })
    .await
}
#[tauri::command]
async fn run_duplicate_check(
    app: AppHandle,
    request_id: String,
    progress: tauri::ipc::Channel<DuplicateProgress>,
) -> Result<DuplicateReport> {
    let (root, flag, prepared) = app.state::<Session>().take_duplicates(&request_id)?;
    background(move || {
        let state = app.state::<Session>();
        // Separate scanner gate: the import gate is never held across file reads.
        let result = match state.duplicate_gate.try_lock() {
            Ok(_gate) => run_duplicate_files(prepared, &flag, |value| {
                let _ = progress.send(value);
            }),
            Err(_) => Err("Eine frühere Duplikatprüfung läuft noch; bitte kurz abwarten".into()),
        };
        let stopped = state.finish_duplicates(&root, &request_id, &flag)?;
        result.map(|mut report| {
            if stopped {
                report.cancelled = true;
                report.complete = false;
                report.groups.clear();
            }
            report
        })
    })
    .await
}
#[tauri::command]
fn cancel_duplicate_check(app: AppHandle, request_id: String) -> Result<()> {
    app.state::<Session>().cancel_duplicates(Some(&request_id))
}

#[tauri::command]
async fn storage_report(
    app: AppHandle,
    root: String,
    request_id: String,
    on_progress: tauri::ipc::Channel<StorageProgress>,
) -> Result<StorageReport> {
    let (root, cancelled) = app
        .state::<Session>()
        .begin_storage(Path::new(&root), &request_id)?;
    background(move || {
        let state = app.state::<Session>();
        let result = match state.storage_gate.try_lock() {
            Ok(_gate) => read_storage_report(&root, &cancelled, |progress| {
                let _ = on_progress.send(progress);
            }),
            Err(_) => Err("Eine frühere Speichermessung läuft noch; bitte kurz abwarten".into()),
        };
        let stopped = state.finish_storage(&root, &request_id, &cancelled)?;
        result.map(|mut report| {
            if stopped {
                report.cancelled = true;
                report.complete = false;
            }
            report
        })
    })
    .await
}

#[tauri::command]
fn cancel_storage_report(app: AppHandle, request_id: String) -> Result<()> {
    app.state::<Session>().cancel_storage(Some(&request_id))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Session::default())
        .invoke_handler(tauri::generate_handler![
            create_library,
            prepare_library_recovery,
            run_library_recovery,
            cancel_library_recovery,
            prepare_duplicate_check,
            run_duplicate_check,
            cancel_duplicate_check,
            storage_report,
            cancel_storage_report,
            open_library,
            get_library,
            select_import_source,
            dropped_import_sources,
            prepare_import_package,
            authorize_prepared_import,
            import_prepared_package,
            cancel_prepared_import,
            import_folder,
            import_zip,
            estimate_zip,
            cancel_import,
            reference_folder,
            relink_source,
            query_catalog,
            catalog_filters,
            create_folder,
            add_to_folder,
            remove_from_folder,
            trash_package,
            trash_file,
            trash_entries,
            prepare_trash_removal,
            remove_prepared_trash,
            discard_trash_removal,
            restore_trash,
            annotate_asset,
            bulk_assets,
            annotate_package,
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
        .expect("Game Asset Library could not start");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};

    #[test]
    fn duplicate_preparation_is_owned_before_work_and_stale_completion_cannot_attach_or_clear_newer(
    ) {
        let directory = std::env::temp_dir().join(format!(
            "duplicate-session-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let source = directory.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("A.wav"), b"abc").unwrap();
        fs::write(source.join("B.wav"), b"abc").unwrap();
        let mut library = Library::create(&directory.join("Library")).unwrap();
        library.import_folder(&source).unwrap();
        let root = library.root().to_owned();
        drop(library);
        let image = fs::read(root.join("catalog.sqlite3")).unwrap();
        let session = Session::default();
        session.set_root(root.clone()).unwrap();
        let (_, stopped) = session.begin_duplicates(&root, "prepare-stop").unwrap();
        let prepared = prepare_duplicate_files(&root, None, &AtomicBool::new(false)).unwrap();
        session.cancel_duplicates(Some("prepare-stop")).unwrap();
        assert!(stopped.load(Ordering::Relaxed));
        assert!(session.begin_duplicates(&root, "prepare-stop").is_err());
        assert!(session
            .finish_duplicate_preparation(&root, "prepare-stop", &stopped, prepared)
            .is_err());
        assert!(session
            .finish_duplicates(&root, "prepare-stop", &stopped)
            .unwrap());
        let (_, old) = session.begin_duplicates(&root, "old").unwrap();
        let prepared = prepare_duplicate_files(&root, None, &old).unwrap();
        let (_, new) = session.begin_duplicates(&root, "new").unwrap();
        assert!(old.load(Ordering::Relaxed));
        assert!(session
            .finish_duplicate_preparation(&root, "old", &old, prepared)
            .is_err());
        assert!(session.finish_duplicates(&root, "old", &old).is_err());
        session.cancel_duplicates(Some("old")).unwrap();
        assert!(!new.load(Ordering::Relaxed));
        let prepared = prepare_duplicate_files(&root, None, &new).unwrap();
        assert_eq!(
            session
                .finish_duplicate_preparation(&root, "new", &new, prepared)
                .unwrap()
                .total,
            2
        );
        let (pinned, flag, prepared) = session.take_duplicates("new").unwrap();
        assert!(session.take_duplicates("new").is_err());
        session.set_root(directory.join("different")).unwrap();
        assert!(flag.load(Ordering::Relaxed));
        let report = run_duplicate_files(prepared, &flag, |_| {}).unwrap();
        assert!(report.cancelled);
        assert!(report.groups.is_empty());
        assert!(session.finish_duplicates(&pinned, "new", &flag).is_err());
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), image);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn duplicate_stop_is_independent_and_actual_audio_writer_cancels_inside_its_gate() {
        let directory = std::env::temp_dir().join(format!(
            "duplicate-audio-session-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = directory.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("A.mp3"),
            include_bytes!("../tests/fixtures/audio/stereo44100.mp3"),
        )
        .unwrap();
        let mut library = Library::create(&directory.join("Library")).unwrap();
        library.import_folder(&source).unwrap();
        let root = library.root().to_owned();
        let session = Session::default();
        session.set_root(root.clone()).unwrap();
        assert!(session.begin_duplicates(&root, "").is_err());
        assert!(session.begin_duplicates(&root, &"x".repeat(129)).is_err());
        assert!(session
            .begin_duplicates(Path::new("/owned/other"), "wrong")
            .is_err());
        {
            let _gate = session.import_gate.lock().unwrap();
            assert!(session.begin_duplicates(&root, "inside-mutation").is_err());
        }
        let (_, storage) = session.begin_storage(&root, "storage").unwrap();
        let (_, audio) = session.begin_audio_analysis("audio").unwrap();
        let (_, stopped) = session.begin_duplicates(&root, "stopped").unwrap();
        let prepared = prepare_duplicate_files(&root, None, &stopped).unwrap();
        session
            .finish_duplicate_preparation(&root, "stopped", &stopped, prepared)
            .unwrap();
        session.cancel_duplicates(Some("stopped")).unwrap();
        assert!(stopped.load(Ordering::Relaxed));
        assert!(session.take_duplicates("stopped").is_err());
        assert!(!storage.load(Ordering::Relaxed));
        assert!(!audio.load(Ordering::Relaxed));
        let (_, duplicates) = session.begin_duplicates(&root, "writer-cancelled").unwrap();
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
            .finish_audio_preparation(&root, "audio", &audio, ids)
            .unwrap();
        let (pinned, flag, files) = session.take_audio_analysis("audio").unwrap();
        let summary = session
            .run_prepared_audio(&library, &pinned, "audio", &flag, files, |_| {
                assert!(duplicates.load(Ordering::Relaxed));
                assert!(storage.load(Ordering::Relaxed));
                assert!(session.begin_duplicates(&root, "during-write").is_err());
            })
            .unwrap();
        assert_eq!(summary.ready, 1);
        assert!(session
            .finish_duplicates(&root, "writer-cancelled", &duplicates)
            .unwrap());
        drop(library);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn storage_jobs_pin_root_and_token_and_never_clear_or_cancel_independent_newer_jobs() {
        let session = Session::default();
        let first = PathBuf::from("/owned/first");
        session.set_root(first.clone()).unwrap();
        assert!(session.begin_storage(&first, "").is_err());
        assert!(session.begin_storage(&first, &"x".repeat(129)).is_err());
        assert!(session
            .begin_storage(Path::new("/owned/other"), "job")
            .is_err());
        {
            let gate = session.import_gate.lock().unwrap();
            assert!(session.begin_storage(&first, "during-mutation").is_err());
            drop(gate);
        }
        let (_, older) = session.begin_storage(&first, "older-token").unwrap();
        assert!(session.begin_storage(&first, "older-token").is_err());
        assert!(!older.load(Ordering::Relaxed));
        let (_, newer) = session.begin_storage(&first, "newer-token").unwrap();
        assert!(older.load(Ordering::Relaxed));
        assert!(session
            .finish_storage(&first, "older-token", &older)
            .is_err());
        assert!(Arc::ptr_eq(
            &session.storage_report.lock().unwrap().as_ref().unwrap().1,
            &newer
        ));
        session.cancel_storage(Some("older-token")).unwrap();
        assert!(!newer.load(Ordering::Relaxed));
        let waveform = Arc::new(AtomicBool::new(false));
        *session.audio_waveform.lock().unwrap() = Some(("independent".into(), waveform.clone()));
        session.cancel_storage(Some("newer-token")).unwrap();
        assert!(session.begin_storage(&first, "newer-token").is_err());
        assert!(newer.load(Ordering::Relaxed));
        assert!(!waveform.load(Ordering::Relaxed));
        assert!(session
            .finish_storage(&first, "newer-token", &newer)
            .unwrap());
        assert!(session.storage_report.lock().unwrap().is_none());
        let (_, root_old) = session.begin_storage(&first, "root-old").unwrap();
        let second = PathBuf::from("/owned/second");
        session.set_root(second.clone()).unwrap();
        assert!(root_old.load(Ordering::Relaxed));
        let (_, latest) = session.begin_storage(&second, "root-new").unwrap();
        assert!(session
            .finish_storage(&first, "root-old", &root_old)
            .is_err());
        assert!(!session
            .finish_storage(&second, "root-new", &latest)
            .unwrap());
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
        let (_, storage_flag) = session.begin_storage(&job_root, "active-storage").unwrap();
        let mut progress = Vec::new();
        let summary = session
            .run_prepared_audio(
                &library,
                &job_root,
                "latest-token",
                &flag,
                frozen,
                |value| {
                    assert!(
                        storage_flag.load(Ordering::Relaxed),
                        "population cancels the report inside its mutation gate before progress or writes"
                    );
                    if value.processed == 1 {
                        session.cancel_audio_analysis(Some("latest-token")).unwrap();
                    }
                    progress.push(value);
                },
            )
            .unwrap();
        assert!(session
            .finish_storage(&job_root, "active-storage", &storage_flag)
            .unwrap());
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
        session
            .pin_preview(&current, request, &snapshot, || Ok(()))
            .unwrap();
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
        session
            .pin_preview(&current, request, &snapshot, || Ok(()))
            .unwrap();
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
                .pin_preview(&active_root, current_request, current, || Ok(()))
                .unwrap();
            assert!(session
                .pin_preview(&active_root, slow_request, slow, || {
                    panic!("Superseded requests must not gain file-protocol access")
                })
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
            let generation = session.model_preview.lock().unwrap().request;
            session.allow_thumbnail(&active_root, || Ok(())).unwrap();
            assert_eq!(session.model_preview.lock().unwrap().request, generation);
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
            .pin_preview(&active_root, old_request, &model, || {
                panic!("A reopened library must invalidate pending resource access")
            })
            .is_err());
        assert!(session
            .export_preview(&model.snapshot_id, model.file_id)
            .is_err());
        session.set_root(root.join("another-library")).unwrap();
        assert!(session
            .allow_thumbnail(&active_root, || {
                panic!("Old-library thumbnails must not gain file-protocol access")
            })
            .is_err());
        drop(library);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "preimport_session_tests.rs"]
mod preimport_session_tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "recovery_session_tests.rs"]
mod recovery_session_tests;
#[derive(Clone, Copy, PartialEq, Eq)]
enum RecoveryPhase {
    Preparing,
    Ready,
    Running,
}
struct RecoveryJob {
    request_id: String,
    root: Option<PathBuf>,
    generation: u64,
    control: Arc<ImportControl>,
    plan: Option<PreparedRecovery>,
    phase: RecoveryPhase,
}
impl Session {
    fn begin_recovery(
        &self,
        root: Option<&Path>,
        id: &str,
    ) -> Result<(Option<PathBuf>, u64, Arc<ImportControl>)> {
        if id.is_empty() || id.len() > 128 || root.is_some_and(|r| r.as_os_str().len() > 4096) {
            return Err("Ungültige Wiederherstellungsanfrage".into());
        }
        let _gate = self
            .import_gate
            .try_lock()
            .map_err(|_| "Bitte den laufenden Dateivorgang abwarten")?;
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != root {
            return Err("Aktive Bibliothek wurde geändert".into());
        }
        let mut jobs = self
            .recovery
            .lock()
            .map_err(|_| "Wiederherstellung ist nicht verfügbar")?;
        if jobs.as_ref().is_some_and(|j| j.request_id == id) {
            return Err("Diese Anfrage ist bereits registriert".into());
        }
        let generation = self.root_generation.load(Ordering::Relaxed);
        let control = Arc::new(ImportControl::default());
        if let Some(old) = jobs.replace(RecoveryJob {
            request_id: id.into(),
            root: current.clone(),
            generation,
            control: control.clone(),
            plan: None,
            phase: RecoveryPhase::Preparing,
        }) {
            old.control.cancel()
        }
        Ok((current.clone(), generation, control))
    }
    fn attach_recovery(
        &self,
        root: Option<&Path>,
        generation: u64,
        id: &str,
        control: &Arc<ImportControl>,
        plan: PreparedRecovery,
    ) -> Result<RecoveryPreparation> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut jobs = self
            .recovery
            .lock()
            .map_err(|_| "Wiederherstellung ist nicht verfügbar")?;
        let job = jobs
            .as_mut()
            .filter(|j| {
                j.request_id == id
                    && Arc::ptr_eq(&j.control, control)
                    && j.phase == RecoveryPhase::Preparing
            })
            .ok_or("Wiederherstellungsanfrage wurde ersetzt")?;
        if current.as_deref() != root
            || self.root_generation.load(Ordering::Relaxed) != generation
            || control.cancellation_flag().load(Ordering::Relaxed)
        {
            return Err("Vorbereitung ist nicht mehr aktuell".into());
        }
        let reply = plan.reply(id);
        job.plan = Some(plan);
        job.phase = RecoveryPhase::Ready;
        Ok(reply)
    }
    fn take_recovery(
        &self,
        id: &str,
    ) -> Result<(Option<PathBuf>, u64, Arc<ImportControl>, PreparedRecovery)> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        let mut jobs = self
            .recovery
            .lock()
            .map_err(|_| "Wiederherstellung ist nicht verfügbar")?;
        let job = jobs
            .as_mut()
            .filter(|j| {
                j.request_id == id
                    && j.phase == RecoveryPhase::Ready
                    && j.root.as_deref() == current.as_deref()
                    && j.generation == self.root_generation.load(Ordering::Relaxed)
                    && !j.control.cancellation_flag().load(Ordering::Relaxed)
            })
            .ok_or("Bitte den Vorgang erneut vorbereiten")?;
        let plan = job.plan.take().ok_or("Plan wurde bereits verbraucht")?;
        job.phase = RecoveryPhase::Running;
        Ok((job.root.clone(), job.generation, job.control.clone(), plan))
    }
    fn with_recovery_root(
        &self,
        root: Option<&Path>,
        generation: u64,
        id: &str,
        control: &Arc<ImportControl>,
        operation: &mut dyn FnMut() -> Result<Option<String>>,
    ) -> Result<Option<String>> {
        let current = self
            .root
            .lock()
            .map_err(|_| "Bibliothek ist nicht verfügbar")?;
        if current.as_deref() != root
            || self.root_generation.load(Ordering::Relaxed) != generation
            || control.cancellation_flag().load(Ordering::Relaxed)
        {
            return Err("Vorgang ist nicht mehr aktuell".into());
        }
        if !self
            .recovery
            .lock()
            .map_err(|_| "Wiederherstellung ist nicht verfügbar")?
            .as_ref()
            .is_some_and(|j| {
                j.request_id == id
                    && Arc::ptr_eq(&j.control, control)
                    && j.phase == RecoveryPhase::Running
            })
        {
            return Err("Wiederherstellungsanfrage wurde ersetzt".into());
        }
        operation()
    }
    fn finish_recovery(&self, id: &str, control: &Arc<ImportControl>) -> Result<()> {
        let mut jobs = self
            .recovery
            .lock()
            .map_err(|_| "Wiederherstellung ist nicht verfügbar")?;
        if jobs
            .as_ref()
            .is_some_and(|j| j.request_id == id && Arc::ptr_eq(&j.control, control))
        {
            jobs.take();
        }
        Ok(())
    }
    fn cancel_recovery(&self, id: Option<&str>) -> Result<()> {
        let mut jobs = self
            .recovery
            .lock()
            .map_err(|_| "Wiederherstellung ist nicht verfügbar")?;
        if let Some(j) = jobs
            .as_mut()
            .filter(|j| id.is_none_or(|id| j.request_id == id))
        {
            j.control.cancel();
            j.plan.take();
            if j.phase == RecoveryPhase::Ready {
                jobs.take();
            }
        }
        Ok(())
    }
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryAcknowledgement {
    request_id: String,
    acknowledged: bool,
}
#[tauri::command]
async fn prepare_library_recovery(
    app: AppHandle,
    root: Option<String>,
    request_id: String,
    request: RecoveryRequest,
    on_progress: tauri::ipc::Channel<RecoveryProgress>,
) -> Result<RecoveryPreparation> {
    match &request {
        RecoveryRequest::Backup { destination_parent } => {
            selected_path(&app, Path::new(destination_parent))?
        }
        RecoveryRequest::Restore {
            artifact_root,
            destination_parent,
        } => {
            selected_path(&app, Path::new(artifact_root))?;
            selected_path(&app, Path::new(destination_parent))?;
        }
    }
    let kind = request.kind();
    let expected = root.as_deref().map(Path::new);
    let (root, generation, control) = app
        .state::<Session>()
        .begin_recovery(expected, &request_id)?;
    background(move || {
        let state = app.state::<Session>();
        let prepared = (|| {
            let _gate = state.import_gate.try_lock().map_err(|_| RecoveryFault {
                reason: "busy",
                message: "Bitte den laufenden Dateivorgang abwarten".into(),
                cancelled: false,
            })?;
            state.cancel_storage(None).map_err(|e| RecoveryFault {
                reason: "busy",
                message: e,
                cancelled: false,
            })?;
            state.cancel_duplicates(None).map_err(|e| RecoveryFault {
                reason: "busy",
                message: e,
                cancelled: false,
            })?;
            prepare_recovery(root.as_deref(), request, &control, &request_id, |p| {
                let _ = on_progress.send(p);
            })
        })();
        match prepared {
            Ok(plan) => match state.attach_recovery(
                root.as_deref(),
                generation,
                &request_id,
                &control,
                plan,
            ) {
                Ok(reply) => Ok(reply),
                Err(e) => {
                    state.finish_recovery(&request_id, &control)?;
                    Ok(RecoveryFault {
                        reason: "stale",
                        message: e,
                        cancelled: false,
                    }
                    .preparation(&request_id, kind, &control))
                }
            },
            Err(e) => {
                state.finish_recovery(&request_id, &control)?;
                Ok(e.preparation(&request_id, kind, &control))
            }
        }
    })
    .await
}
#[tauri::command]
async fn run_library_recovery(
    app: AppHandle,
    request_id: String,
    on_progress: tauri::ipc::Channel<RecoveryProgress>,
) -> Result<RecoveryOutcome> {
    let (root, generation, control, plan) = app.state::<Session>().take_recovery(&request_id)?;
    background(move || {
        let state = app.state::<Session>();
        let destination = plan.destination.clone();
        let kind = plan.kind;
        let result = (|| -> Result<RecoveryOutcome> {
            Ok(match state.import_gate.try_lock() {
                Ok(_gate) => {
                    state.cancel_storage(None)?;
                    state.cancel_duplicates(None)?;
                    let mut publisher = |op: &mut dyn FnMut() -> Result<Option<String>>| {
                        state.with_recovery_root(
                            root.as_deref(),
                            generation,
                            &request_id,
                            &control,
                            op,
                        )
                    };
                    run_recovery(
                        plan,
                        &control,
                        &request_id,
                        |p| {
                            let _ = on_progress.send(p);
                        },
                        &mut publisher,
                    )
                }
                Err(_) => RecoveryOutcome::Failed {
                    request_id: request_id.clone(),
                    kind,
                    reason: "busy",
                    message: "Bitte den laufenden Dateivorgang abwarten und neu vorbereiten".into(),
                    cleanup: "notNeeded",
                    residue_path: None,
                },
            })
        })();
        let mut result = result.unwrap_or_else(|e| RecoveryOutcome::Failed {
            request_id: request_id.clone(),
            kind,
            reason: "busy",
            message: e,
            cleanup: "notNeeded",
            residue_path: None,
        });
        if let Err(e) = state.finish_recovery(&request_id, &control) {
            if let RecoveryOutcome::Published {
                warning,
                durability,
                ..
            } = &mut result
            {
                *warning = Some(format!(
                    "{} Anfragefreigabe nach Veröffentlichung fehlgeschlagen: {e}",
                    warning.take().unwrap_or_default()
                ));
                *durability = "uncertain";
            }
        }
        if kind == RecoveryKind::Restore {
            if let RecoveryOutcome::Published {
                destination_path,
                warning,
                durability,
                ..
            } = &mut result
            {
                if Path::new(destination_path) == destination {
                    if let Err(e) = app
                        .asset_protocol_scope()
                        .allow_directory(&destination, true)
                    {
                        *warning = Some(format!(
                            "{} Freigabe für späteres Öffnen fehlgeschlagen: {e}",
                            warning.take().unwrap_or_default()
                        ));
                        *durability = "uncertain";
                    }
                }
            }
        }
        Ok(result)
    })
    .await
}
#[tauri::command]
fn cancel_library_recovery(app: AppHandle, request_id: String) -> Result<RecoveryAcknowledgement> {
    app.state::<Session>().cancel_recovery(Some(&request_id))?;
    Ok(RecoveryAcknowledgement {
        request_id,
        acknowledged: true,
    })
}
