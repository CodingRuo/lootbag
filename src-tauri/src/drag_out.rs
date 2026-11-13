use super::{
    model_resources::SourceStamp,
    project_export::{companion_paths, unique_name},
    unique_suffix, AssetKind, ImageWorkflow, Library, Result,
};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
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
    /// Staged files stay until the caller clears `staging` (next launch or library open).
    pub fn stage_drag(&self, sources: &[DragSource], staging: &Path) -> Result<DragStage> {
        if sources.is_empty() || sources.len() > MAX_SOURCES {
            return Err("Bitte 1 bis 10.000 Assets zum Herausziehen auswählen".into());
        }
        let mut staging = Staging {
            library: self,
            batch: staging.join(unique_suffix()),
            used: HashSet::new(),
            seen: HashSet::new(),
            originals: Vec::new(),
            stage: DragStage::default(),
        };
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
            return Err(if stage.problems.is_empty() {
                "Nichts zum Herausziehen vorbereitet".into()
            } else {
                stage.problems.join("; ")
            });
        }
        Ok(stage)
    }
}

impl Staging<'_> {
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
                    self.model(file_id)
                } else {
                    self.plain(file_id)
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

    fn model(&mut self, file_id: i64) -> Result<()> {
        let snapshot = self.library.model_resources(file_id)?;
        let (extra, notes) = companion_paths(&snapshot)?;
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
            self.library.export_model(&snapshot, &extra, None)?;
            return self.plain(file_id);
        }
        let stem = entry
            .file_stem()
            .map_or("Modell".into(), |stem| stem.to_string_lossy());
        let name = unique_name(&mut self.used, "", &stem, "");
        let destination = self.batch()?.join(name);
        // Rejects incomplete models and re-checks every resource stamp while copying.
        self.library
            .export_model(&snapshot, &extra, Some(&destination))?;
        self.stage.items.push(DragItem {
            path: destination,
            directory: true,
        });
        Ok(())
    }

    fn plain(&mut self, file_id: i64) -> Result<()> {
        let path = self.library.resolve_preview(file_id)?;
        let mut input =
            fs::File::open(&path).map_err(|error| format!("Datei nicht lesbar: {error}"))?;
        let stamp = SourceStamp::read(&input.metadata().map_err(|error| error.to_string())?)?;
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

/// Ties a prepared drag payload to the mouse press it was prepared for. A drag starts only from
/// a drag event of that same, still held press; release, Escape or any invalidation disarms it.
#[derive(Debug)]
pub struct DragGesture<P> {
    press: Option<Press<P>>,
    presses: u64,
}

#[derive(Debug)]
struct Press<P> {
    id: u64,
    origin: (f64, f64),
    payload: Option<P>,
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
    /// A new mouse-down; whatever an earlier press had armed is dropped.
    pub fn press(&mut self, at: (f64, f64)) {
        self.presses += 1;
        self.press = Some(Press {
            id: self.presses,
            origin: at,
            payload: None,
        });
    }

    /// The press still held, so a preparation can be bound to it.
    pub fn held(&self) -> Option<u64> {
        self.press.as_ref().map(|press| press.id)
    }

    /// Arms `press` with `payload`; false once that press ended or was replaced.
    pub fn arm(&mut self, press: u64, payload: P) -> bool {
        match &mut self.press {
            Some(held) if held.id == press => {
                held.payload = Some(payload);
                true
            }
            _ => false,
        }
    }

    /// A drag event of the held press: past the threshold and armed, the payload is handed over
    /// once and the press ends, so it can never start twice.
    pub fn drag(&mut self, at: (f64, f64)) -> Option<P> {
        let press = self.press.as_ref()?;
        let (dx, dy) = (at.0 - press.origin.0, at.1 - press.origin.1);
        if press.payload.is_none() || dx.hypot(dy) < DRAG_THRESHOLD {
            return None;
        }
        self.press.take()?.payload
    }

    /// Mouse-up, Escape, selection or library change.
    pub fn disarm(&mut self) {
        self.press = None;
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
