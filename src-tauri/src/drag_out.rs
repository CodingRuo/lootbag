use super::{
    model_resources::{ModelResources, SourceStamp},
    project_export::{companion_paths, unique_name},
    unique_suffix, AssetKind, ImageWorkflow, Library, Result,
};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{channel, Sender},
};

const MAX_SOURCES: usize = 10_000;
/// Total size of staged copies in the drag-out folder; published copies stay until the next launch.
pub const DRAG_CACHE_BUDGET: u64 = 2 * 1024 * 1024 * 1024;
const CACHE_FULL: &str = "Der Zwischenspeicher für herausgezogene Kopien ist voll; er wird beim nächsten Start von Lootbag geleert";

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
    /// Bytes this drag may still copy; a copy beyond it refuses the whole drag.
    budget: u64,
    full: bool,
}

impl Library {
    /// Prepares a drag-out: originals where their names are unique, copies under `staging` for
    /// repeated names, and one bundle folder per model with companions, built by the model export. Each item is
    /// prepared on its own; failures and source changes become `problems` instead of aborting.
    /// A failed preparation leaves nothing behind; otherwise `batch` names what was copied.
    pub fn stage_drag(&self, sources: &[DragSource], staging: &Path) -> Result<DragStage> {
        self.stage_drag_within(sources, staging, DRAG_CACHE_BUDGET)
    }

    /// `stage_drag` with an explicit cache budget for everything below `staging`.
    pub fn stage_drag_within(
        &self,
        sources: &[DragSource],
        staging: &Path,
        budget: u64,
    ) -> Result<DragStage> {
        if sources.is_empty() || sources.len() > MAX_SOURCES {
            return Err("Bitte 1 bis 10.000 Assets zum Herausziehen auswählen".into());
        }
        let mut staging = Staging::new(self, staging, budget.saturating_sub(dir_size(staging)));
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
        if staging.full {
            if let Some(batch) = &stage.batch {
                let _ = fs::remove_dir_all(batch);
            }
            return Err(CACHE_FULL.into());
        }
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
    fn new(library: &'a Library, staging: &Path, budget: u64) -> Self {
        Self {
            library,
            batch: staging.join(unique_suffix()),
            used: HashSet::new(),
            seen: HashSet::new(),
            originals: Vec::new(),
            stage: DragStage::default(),
            budget,
            full: false,
        }
    }

    /// Claims `bytes` of the cache budget for a copy.
    fn reserve(&mut self, bytes: u64) -> Result<()> {
        if self.full || bytes > self.budget {
            self.full = true;
            return Err(CACHE_FULL.into());
        }
        self.budget -= bytes;
        Ok(())
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
        let size = snapshot.export_size(&extra);
        self.reserve(size)?;
        let name = unique_name(&mut self.used, "", &stem, "");
        let destination = self.batch.join(name);
        // Rejects incomplete models and re-checks every resource stamp while copying; a failed
        // export removes its own partial output.
        let exported = self.batch().and_then(|_| {
            self.library
                .export_model(snapshot, &extra, Some(&destination))
        });
        if let Err(error) = exported {
            self.budget += size;
            return Err(error);
        }
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
        let size = input.metadata().map_err(|error| error.to_string())?.len();
        self.reserve(size)?;
        let target = self.batch.join(name);
        let copied = (|| {
            self.batch()?;
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
            self.budget += size;
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

/// A point in a view's own coordinates as a CSS pixel in its viewport (top-left origin), given
/// the view's height and whether it is flipped. Assumes page zoom 1, so one point is one pixel.
pub fn css_point((x, y): (f64, f64), height: f64, flipped: bool) -> (f64, f64) {
    (x, if flipped { y } else { height - y })
}

/// Ties a drag-out to the mouse press it belongs to. Once a held press passes the threshold, the
/// pressed window's page is asked which card lies under the press point (while the button is
/// still down, so its DOM is what was pressed); only an answer for that same, still held press in
/// that window is staged, once. Nothing is staged on a mere click, and only a drag event of the
/// same held press starts the drag.
#[derive(Debug)]
pub struct DragGesture<P> {
    press: Option<Press<P>>,
    presses: u64,
}

#[derive(Debug)]
struct Press<P> {
    id: u64,
    window: usize,
    origin: (f64, f64),
    /// The press in the pressed webview's viewport; `None` if it missed the webview.
    css: Option<(f64, f64)>,
    state: PressState<P>,
}

#[derive(Debug)]
enum PressState<P> {
    Pressed,
    Querying,
    Staging,
    /// Nothing to drag: the press missed every card, or its payload was handed over.
    Idle,
    Armed(P),
}

/// What a drag event asks the caller to do.
#[derive(Debug, PartialEq)]
pub enum DragStep<P> {
    Wait,
    /// Ask `window`'s page which card lies under the press point (viewport CSS pixels); once
    /// per press.
    Query {
        press: u64,
        window: usize,
        at: (f64, f64),
    },
    Start(P),
}

impl<P> Default for DragGesture<P> {
    fn default() -> Self {
        Self {
            press: None,
            presses: 0,
        }
    }
}

impl<P> DragGesture<P> {
    /// A new mouse-down in `window` at `at` (window points); `css` is the press in the pressed
    /// webview's viewport, if it hit one. Returns whatever an earlier press had armed.
    pub fn press(&mut self, window: usize, at: (f64, f64), css: Option<(f64, f64)>) -> Option<P> {
        let dropped = self.disarm();
        self.presses += 1;
        self.press = Some(Press {
            id: self.presses,
            window,
            origin: at,
            css,
            state: if css.is_some() {
                PressState::Pressed
            } else {
                PressState::Idle
            },
        });
        dropped
    }

    /// The page's answer for `press` in `window`: true if `files` should now be staged for it.
    /// A wrong window or press, a released press or a repeated answer is refused.
    pub fn answer<S>(&mut self, window: usize, press: u64, files: &Option<S>) -> bool {
        let Some(held) = &mut self.press else {
            return false;
        };
        if held.id != press || held.window != window || !matches!(held.state, PressState::Querying)
        {
            return false;
        }
        held.state = if files.is_some() {
            PressState::Staging
        } else {
            PressState::Idle
        };
        files.is_some()
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

    /// A drag event of the held press. Past the threshold a fresh press asks its page once;
    /// an armed one hands its payload over once and ends, so it can never start twice.
    pub fn drag(&mut self, at: (f64, f64)) -> DragStep<P> {
        let Some(held) = &mut self.press else {
            return DragStep::Wait;
        };
        let (dx, dy) = (at.0 - held.origin.0, at.1 - held.origin.1);
        if dx.hypot(dy) < DRAG_THRESHOLD {
            return DragStep::Wait;
        }
        match std::mem::replace(&mut held.state, PressState::Idle) {
            PressState::Pressed => {
                held.state = PressState::Querying;
                match held.css {
                    Some(css) => DragStep::Query {
                        press: held.id,
                        window: held.window,
                        at: css,
                    },
                    None => DragStep::Wait,
                }
            }
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

/// Deletes staged batches on a worker thread: AppKit callbacks and the event monitor must never
/// wait on removing a large folder.
#[derive(Debug)]
pub struct Trash(Sender<PathBuf>);

impl Trash {
    /// A trash whose removals are handled by `sender`'s receiver.
    pub fn new(sender: Sender<PathBuf>) -> Self {
        Self(sender)
    }

    /// A trash with its own worker that removes each folder it is handed.
    pub fn spawn() -> Self {
        let (sender, removals) = channel::<PathBuf>();
        std::thread::spawn(move || {
            for path in removals {
                let _ = fs::remove_dir_all(path);
            }
        });
        Self(sender)
    }

    /// Queues `stage`'s copies for removal; returns at once.
    pub fn discard(&self, stage: &DragStage) {
        if let Some(batch) = &stage.batch {
            self.remove(batch.clone());
        }
    }

    pub fn remove(&self, path: PathBuf) {
        let _ = self.0.send(path);
    }
}

/// Launch cleanup: moves the drag-out folder `staging` aside (a rename, so new drags can stage at
/// once) and returns it with any folders an earlier launch moved aside but did not finish removing.
/// Published copies live until here, because a recipient may read them long after the drop.
pub fn retire_drag_cache(staging: &Path) -> Vec<PathBuf> {
    let (Some(parent), Some(name)) = (staging.parent(), staging.file_name()) else {
        return Vec::new();
    };
    let prefix = format!("{}-old-", name.to_string_lossy());
    let _ = fs::rename(staging, parent.join(format!("{prefix}{}", unique_suffix())));
    fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect()
}

/// Bytes of all regular files below `dir`, not following links.
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).into_iter().flatten().flatten() {
            match entry.metadata() {
                Ok(metadata) if metadata.is_dir() => pending.push(entry.path()),
                Ok(metadata) => total += metadata.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "drag_out_tests.rs"]
mod tests;
