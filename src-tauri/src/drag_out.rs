use super::{
    model_resources::{ModelResources, SourceStamp},
    project_export::{companion_paths, unique_name},
    unique_suffix, AssetKind, ImageWorkflow, Library, Result,
};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_SOURCES: usize = 10_000;

/// One dragged catalog item: a file, or with `workflow` the whole image group it represents.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragSource {
    pub file_id: i64,
    pub workflow: Option<ImageWorkflow>,
}

/// One file or bundle folder on the drag pasteboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragItem {
    pub path: PathBuf,
    /// Bundle folders must be announced as directories, or Finder treats them as files.
    pub directory: bool,
}

/// What a native drag hands over, plus a German line for every item left out or changed.
#[derive(Debug, Default)]
pub struct DragStage {
    pub items: Vec<DragItem>,
    pub problems: Vec<String>,
    /// This drag's own staging folder, if anything was copied; the caller removes it once unused.
    pub batch: Option<PathBuf>,
}

struct Staging<'a> {
    library: &'a Library,
    /// This drag's own folder below the staging root, created on first use.
    batch: PathBuf,
    /// Top-level names already handed over, lowercase (Finder and Explorer compare case-insensitively).
    used: HashSet<String>,
    seen: HashSet<i64>,
    originals: Vec<(PathBuf, SourceStamp)>,
    stage: DragStage,
}

impl Library {
    /// Prepares a drag-out: originals where their names are unique, copies under `staging` for
    /// repeated names, and one bundle folder per model with companions, built by the model export. Each item is
    /// prepared on its own; failures and source changes become `problems` instead of aborting.
    /// A failed preparation leaves nothing behind; otherwise `batch` names what was copied.
    pub fn stage_drag(&self, sources: &[DragSource], staging: &Path) -> Result<DragStage> {
        if sources.is_empty() || sources.len() > MAX_SOURCES {
            return Err("Bitte 1 bis 10.000 Assets zum Herausziehen auswählen".into());
        }
        let mut staging = Staging::new(self, staging);
        for source in sources {
            match source.workflow {
                Some(workflow) => match self.image_group(source.file_id, workflow) {
                    Ok(group) => {
                        let name = group.name;
                        staging.stage.problems.extend(
                            group
                                .errors
                                .into_iter()
                                .map(|error| format!("{name}: {error}")),
                        );
                        for member in group.members {
                            staging.file(member.file.id);
                        }
                    }
                    Err(error) => staging.stage.problems.push(error),
                },
                None => staging.file(source.file_id),
            }
        }
        // Originals are handed over as they are, so a change after they were resolved drops them.
        let mut stage = staging.stage;
        stage.batch = staging.batch.is_dir().then_some(staging.batch);
        for (path, stamp) in staging.originals {
            let unchanged = fs::metadata(&path)
                .ok()
                .and_then(|metadata| SourceStamp::read(&metadata).ok())
                .is_some_and(|now| now == stamp);
            if !unchanged {
                stage.items.retain(|item| item.path != path);
                stage.problems.push(format!(
                    "{}: wurde während der Vorbereitung geändert",
                    file_name(&path)
                ));
            }
        }
        if stage.items.is_empty() {
            if let Some(batch) = &stage.batch {
                let _ = fs::remove_dir_all(batch);
            }
            return Err(if stage.problems.is_empty() {
                "Nichts zum Herausziehen vorbereitet".into()
            } else {
                stage.problems.join("; ")
            });
        }
        Ok(stage)
    }
}

impl<'a> Staging<'a> {
    fn new(library: &'a Library, staging: &Path) -> Self {
        Self {
            library,
            batch: staging.join(unique_suffix()),
            used: HashSet::new(),
            seen: HashSet::new(),
            originals: Vec::new(),
            stage: DragStage::default(),
        }
    }

    fn file(&mut self, file_id: i64) {
        if !self.seen.insert(file_id) {
            return;
        }
        let result = self
            .library
            .connection
            .query_row(
                "SELECT format, original_filename FROM active_asset_files WHERE id = ?1",
                [file_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("Datei nicht im aktiven Katalog: {error}"))
            .and_then(|(format, filename)| {
                let model =
                    AssetKind::for_format(&format) == Some(AssetKind::Model) && format != "BLEND";
                if model {
                    self.library
                        .model_resources(file_id)
                        .and_then(|snapshot| self.model(file_id, &snapshot))
                } else {
                    self.plain(file_id, None)
                }
                .map_err(|error| format!("{filename}: {error}"))
            });
        if let Err(error) = result {
            self.stage.problems.push(error);
        }
    }

    fn batch(&self) -> Result<&Path> {
        fs::create_dir_all(&self.batch)
            .map_err(|error| format!("Zwischenablage-Ordner nicht anlegbar: {error}"))?;
        Ok(&self.batch)
    }

    fn model(&mut self, file_id: i64, snapshot: &ModelResources) -> Result<()> {
        let (extra, notes) = companion_paths(snapshot)?;
        let entry = Path::new(&snapshot.entry_path);
        let entry_name = file_name(entry);
        self.stage.problems.extend(
            notes
                .into_iter()
                .map(|note| format!("{entry_name}: {note}")),
        );
        // Same export set as export_model: entry, declared resources and detected extras.
        let bundled = snapshot
            .declared_paths
            .iter()
            .chain(&extra)
            .any(|path| *path != snapshot.entry_path);
        if !bundled {
            // Without a destination the export only validates (missing resources, stamps).
            self.library.export_model(snapshot, &extra, None)?;
            // The handed-over file must still be the one whose companions were checked.
            return self.plain(file_id, Some(snapshot.entry_stamp()?));
        }
        let stem = entry
            .file_stem()
            .map_or("Modell".into(), |stem| stem.to_string_lossy());
        let name = unique_name(&mut self.used, "", &stem, "");
        let destination = self.batch()?.join(name);
        // Rejects incomplete models and re-checks every resource stamp while copying.
        self.library
            .export_model(snapshot, &extra, Some(&destination))?;
        self.stage.items.push(DragItem {
            path: destination,
            directory: true,
        });
        Ok(())
    }

    fn plain(&mut self, file_id: i64, expected: Option<&SourceStamp>) -> Result<()> {
        let path = self.library.resolve_preview(file_id)?;
        let mut input =
            fs::File::open(&path).map_err(|error| format!("Datei nicht lesbar: {error}"))?;
        let stamp = SourceStamp::read(&input.metadata().map_err(|error| error.to_string())?)?;
        if expected.is_some_and(|expected| *expected != stamp) {
            return Err("wurde während der Vorbereitung geändert".into());
        }
        let original = file_name(&path);
        let (stem, extension) = match original.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
            _ => (original.as_str(), String::new()),
        };
        let name = unique_name(&mut self.used, "", stem, &extension);
        if name == original {
            self.originals.push((path.clone(), stamp));
            self.stage.items.push(DragItem {
                path,
                directory: false,
            });
            return Ok(());
        }
        // A repeated name gets a renamed copy, so the recipient never sees two items called the same.
        let target = self.batch()?.join(name);
        let copied = (|| {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|error| format!("Kopie nicht anlegbar: {error}"))?;
            io::copy(&mut input, &mut output)
                .map_err(|error| format!("Kopie fehlgeschlagen: {error}"))?;
            if SourceStamp::read(&input.metadata().map_err(|error| error.to_string())?)? != stamp {
                return Err("wurde während der Vorbereitung geändert".to_string());
            }
            Ok(())
        })();
        if let Err(error) = copied {
            let _ = fs::remove_file(&target);
            return Err(error);
        }
        self.stage.items.push(DragItem {
            path: target,
            directory: false,
        });
        Ok(())
    }
}

/// Pointer travel before a held press becomes a drag, in points.
pub const DRAG_THRESHOLD: f64 = 6.0;

/// Ties a drag-out to the mouse press it was requested for. The monitor numbers every press
/// and tells the pressed webview that number before WebKit sees the event, so the frontend's
/// request names its own press; a request or payload for any other press or window is refused.
/// Nothing is staged on a mere click: staging starts once the held press passes the threshold,
/// and only a drag event of that same, still held press starts the drag.
#[derive(Debug)]
pub struct DragGesture<S, P> {
    press: Option<Press<S, P>>,
    presses: u64,
}

#[derive(Debug)]
struct Press<S, P> {
    id: u64,
    window: usize,
    origin: (f64, f64),
    state: PressState<S, P>,
}

#[derive(Debug)]
enum PressState<S, P> {
    Pressed,
    Requested(S),
    Staging,
    Armed(P),
}

/// What a drag event asks the caller to do.
#[derive(Debug, PartialEq, Eq)]
pub enum DragStep<S, P> {
    Wait,
    /// Stage `S` for press `u64`, then `arm` it; asked once per press.
    Stage(u64, S),
    Start(P),
}

impl<S, P> Default for DragGesture<S, P> {
    fn default() -> Self {
        Self {
            press: None,
            presses: 0,
        }
    }
}

impl<S, P> DragGesture<S, P> {
    /// A new mouse-down in `window`; returns its number and whatever an earlier press had armed.
    pub fn press(&mut self, window: usize, at: (f64, f64)) -> (u64, Option<P>) {
        self.presses += 1;
        let dropped = self.disarm();
        self.press = Some(Press {
            id: self.presses,
            window,
            origin: at,
            state: PressState::Pressed,
        });
        (self.presses, dropped)
    }

    /// Binds `sources` to `press` if it is the held press of `window` and has none yet.
    pub fn request(&mut self, window: usize, press: u64, sources: S) -> bool {
        match &mut self.press {
            Some(held)
                if held.id == press
                    && held.window == window
                    && matches!(held.state, PressState::Pressed) =>
            {
                held.state = PressState::Requested(sources);
                true
            }
            _ => false,
        }
    }

    /// Arms `press` with its staged `payload`; hands the payload back once that press ended.
    pub fn arm(&mut self, press: u64, payload: P) -> std::result::Result<(), P> {
        match &mut self.press {
            Some(held) if held.id == press && matches!(held.state, PressState::Staging) => {
                held.state = PressState::Armed(payload);
                Ok(())
            }
            _ => Err(payload),
        }
    }

    /// A drag event of the held press. Past the threshold a requested press asks for staging
    /// once; an armed one hands its payload over once and ends, so it can never start twice.
    pub fn drag(&mut self, at: (f64, f64)) -> DragStep<S, P> {
        let Some(held) = &mut self.press else {
            return DragStep::Wait;
        };
        let (dx, dy) = (at.0 - held.origin.0, at.1 - held.origin.1);
        if dx.hypot(dy) < DRAG_THRESHOLD {
            return DragStep::Wait;
        }
        match std::mem::replace(&mut held.state, PressState::Staging) {
            PressState::Requested(sources) => DragStep::Stage(held.id, sources),
            PressState::Armed(payload) => {
                self.press = None;
                DragStep::Start(payload)
            }
            other => {
                held.state = other;
                DragStep::Wait
            }
        }
    }

    /// Mouse-up, Escape, selection or library change; returns a payload that was never used.
    pub fn disarm(&mut self) -> Option<P> {
        match self.press.take()?.state {
            PressState::Armed(payload) => Some(payload),
            _ => None,
        }
    }
}

/// Published staging batches stay this long after their drag ended, so a recipient that copies
/// slowly after the drop can still read them.
pub const BATCH_GRACE: Duration = Duration::from_secs(10 * 60);
/// The most recent published batches are kept regardless of age.
pub const KEEP_BATCHES: usize = 3;

/// Bounds staged drag copies: a batch is deleted once it is not among the `KEEP_BATCHES` newest
/// and its drag ended `BATCH_GRACE` ago. Library open still clears everything.
#[derive(Debug, Default)]
pub struct Retention {
    batches: Vec<(PathBuf, Option<Instant>)>,
}

impl Retention {
    pub const fn new() -> Self {
        Self {
            batches: Vec::new(),
        }
    }

    /// A batch handed to a running drag.
    pub fn published(&mut self, batch: PathBuf) {
        self.batches.push((batch, None));
    }

    /// Records that `batch`'s drag ended at `now`; returns the batches now safe to delete.
    pub fn ended(&mut self, batch: Option<&Path>, now: Instant) -> Vec<PathBuf> {
        for (path, end) in &mut self.batches {
            if Some(path.as_path()) == batch {
                end.get_or_insert(now);
            }
        }
        let old = self.batches.len().saturating_sub(KEEP_BATCHES);
        let (mut expired, mut kept) = (Vec::new(), Vec::new());
        for (index, (path, end)) in self.batches.drain(..).enumerate() {
            if index < old && end.is_some_and(|end| now.duration_since(end) >= BATCH_GRACE) {
                expired.push(path);
            } else {
                kept.push((path, end));
            }
        }
        self.batches = kept;
        expired
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "drag_out_tests.rs"]
mod tests;
