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
        self.reserve(snapshot.export_size(&extra))?;
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
        self.reserve(input.metadata().map_err(|error| error.to_string())?.len())?;
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

/// The card under the pointer as the frontend last reported it: what it would drag and where it
/// is, in CSS pixels relative to the webview's viewport.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct DragTarget<S> {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub sources: S,
}

impl<S> DragTarget<S> {
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.left && x < self.left + self.width && y >= self.top && y < self.top + self.height
    }
}

/// A point in a view's own coordinates as a CSS pixel in its viewport (top-left origin), given
/// the view's height and whether it is flipped. Assumes page zoom 1, so one point is one pixel.
pub fn css_point((x, y): (f64, f64), height: f64, flipped: bool) -> (f64, f64) {
    (x, if flipped { y } else { height - y })
}

/// Ties a drag-out to the mouse press it belongs to. The frontend reports the card under the
/// pointer per window; a native mouse-down binds that card only if the press lies inside the
/// card's reported rect, so a report that is late, stale or for another card binds nothing.
/// Nothing is staged on a mere click: staging starts once the held press passes the threshold,
/// and only a drag event of that same, still held press starts the drag.
#[derive(Debug)]
pub struct DragGesture<S, P> {
    press: Option<Press<S, P>>,
    presses: u64,
    /// Per window: the newest report's version and its target, if the pointer is on a card.
    targets: Vec<(usize, f64, Option<DragTarget<S>>)>,
}

#[derive(Debug)]
struct Press<S, P> {
    id: u64,
    origin: (f64, f64),
    state: PressState<S, P>,
}

#[derive(Debug)]
enum PressState<S, P> {
    Pressed,
    Bound(S),
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
            targets: Vec::new(),
        }
    }
}

impl<S: Clone, P> DragGesture<S, P> {
    /// Records `window`'s card under the pointer (`None`: none); an older `version` is ignored.
    pub fn hover(&mut self, window: usize, version: f64, target: Option<DragTarget<S>>) -> bool {
        match self.targets.iter_mut().find(|entry| entry.0 == window) {
            Some(entry) if entry.1 >= version => false,
            Some(entry) => {
                *entry = (window, version, target);
                true
            }
            None => {
                self.targets.push((window, version, target));
                true
            }
        }
    }

    /// A new mouse-down in `window` at `at` (window points); `css` is the press in the pressed
    /// webview's viewport, if it hit one. Binds that window's card if the press lies inside it
    /// and returns whatever an earlier press had armed.
    pub fn press(&mut self, window: usize, at: (f64, f64), css: Option<(f64, f64)>) -> Option<P> {
        let dropped = self.disarm();
        let bound = self
            .targets
            .iter()
            .find(|entry| entry.0 == window)
            .and_then(|entry| entry.2.as_ref())
            .filter(|target| css.is_some_and(|css| target.contains(css)))
            .map(|target| target.sources.clone());
        self.presses += 1;
        self.press = Some(Press {
            id: self.presses,
            origin: at,
            state: bound.map_or(PressState::Pressed, PressState::Bound),
        });
        dropped
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

    /// A drag event of the held press. Past the threshold a bound press asks for staging
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
            PressState::Bound(sources) => DragStep::Stage(held.id, sources),
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
