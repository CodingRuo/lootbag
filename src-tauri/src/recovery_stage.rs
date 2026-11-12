use crate::library::{
    sprite_export::anchored as output,
    storage::{anchored, Stamp},
    unique_suffix, ImportControl, Result,
};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File},
    path::{Component, Path, PathBuf},
    sync::atomic::Ordering,
};

type Identity = (u64, u64);
struct Entry {
    identity: Identity,
    directory: bool,
    children: usize,
    finished: Option<Stamp>,
}

pub(super) struct Stage<'a> {
    staged_root: PathBuf,
    final_leaf: String,
    stage_leaf: String,
    parent: File,
    directory: File,
    parent_identity: Identity,
    identity: Identity,
    ancestors: Vec<(PathBuf, Identity)>,
    entries: BTreeMap<String, Entry>,
    children: usize,
    max_entries: usize,
    max_retained: usize,
    retained: usize,
    unproven: bool,
    synced: bool,
    published: bool,
    cleaned: bool,
    control: &'a ImportControl,
}

fn check(control: &ImportControl) -> Result<()> {
    if control.cancellation_flag().load(Ordering::Relaxed) {
        Err("Wiederherstellungsvorgang wurde abgebrochen".into())
    } else {
        Ok(())
    }
}
fn relative(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 4096 || path.split('/').count() > 64 {
        return Err("Ungültiger oder zu langer Ausgabepfad".into());
    }
    if path.split('/').any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > 255
            || part.contains(['\\', '\0'])
    }) || !Path::new(path)
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("Ausgabepfad ist nicht relativ und sicher".into());
    }
    Ok(())
}
fn charge(retained: &mut usize, maximum: usize, bytes: usize) -> Result<()> {
    let next = retained
        .checked_add(bytes)
        .ok_or("Ausgabe-Speicherlimit überschritten")?;
    if next > maximum {
        return Err("Ausgabe-Speicherlimit überschritten".into());
    }
    *retained = next;
    Ok(())
}
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

impl<'a> Stage<'a> {
    pub(super) fn create(
        parent_path: &Path,
        final_leaf: &str,
        control: &'a ImportControl,
        max_entries: usize,
        max_retained: usize,
    ) -> Result<Self> {
        check(control)?;
        relative(final_leaf)?;
        if final_leaf.contains('/')
            || !parent_path.is_absolute()
            || parent_path.to_str().is_none()
            || parent_path.as_os_str().len() > 4096
            || !parent_path
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err("Ungültiger Wiederherstellungs-Zielordner".into());
        }
        if max_entries == 0 {
            return Err("Ausgabe-Anzahllimit überschritten".into());
        }
        // Conservative map/path overhead and bounded transient listing/path buffers are charged before I/O.
        let mut retained = 0;
        charge(
            &mut retained,
            max_retained,
            16384
                + std::mem::size_of::<Self>()
                + parent_path.as_os_str().len() * 4
                + final_leaf.len()
                + 128,
        )?;
        let mut ancestors = Vec::new();
        let mut current = PathBuf::from("/");
        for part in parent_path.components() {
            check(control)?;
            if let Component::Normal(name) = part {
                current.push(name);
            }
            charge(
                &mut retained,
                max_retained,
                current.as_os_str().len() * 2 + 256,
            )?;
            let metadata = fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("Zielordner oder ein Elternordner wurde ersetzt".into());
            }
            ancestors.push((current.clone(), Stamp::read(&metadata).identity()));
        }
        let parent = anchored::root(parent_path)?;
        let parent_identity =
            Stamp::read(&parent.metadata().map_err(|e| e.to_string())?).identity();
        if ancestors.last().map(|(_, id)| *id) != Some(parent_identity) {
            return Err("Wiederherstellungs-Zielordner wurde ersetzt".into());
        }
        if anchored::optional_child_stamp(&parent, OsStr::new(final_leaf))?.is_some() {
            return Err("Das Wiederherstellungsziel existiert bereits".into());
        }
        check(control)?;
        let stage_leaf = format!(".game-assets-recovery-{}", unique_suffix());
        let staged_root = parent_path.join(&stage_leaf);
        let directory = output::create_at_report_retained(&parent, &stage_leaf, || {
            control.mark_cleanup_failed()
        })
        .map_err(|e| format!("{e} ({})", staged_root.display()))?;
        let identity = match directory.metadata() {
            Ok(metadata) => Stamp::read(&metadata).identity(),
            Err(error) => {
                control.mark_cleanup_failed();
                return Err(format!(
                    "{error}; Zwischenordner wurde nicht sicher entfernt: {}",
                    staged_root.display()
                ));
            }
        };
        let mut stage = Self {
            staged_root,
            final_leaf: final_leaf.into(),
            stage_leaf,
            parent,
            directory,
            parent_identity,
            identity,
            ancestors,
            entries: BTreeMap::new(),
            children: 0,
            max_entries,
            max_retained,
            retained,
            unproven: false,
            synced: false,
            published: false,
            cleaned: false,
            control,
        };
        if let Err(error) = stage.root_check().and_then(|_| check(control)) {
            return Err(if stage.cleanup() {
                error
            } else {
                format!(
                    "{error}; Zwischenordner wurde nicht vollständig entfernt: {}",
                    stage.staged_root.display()
                )
            });
        }
        Ok(stage)
    }
    pub(super) fn directory(&self) -> &File {
        &self.directory
    }
    pub(super) fn staged_root(&self) -> &Path {
        &self.staged_root
    }
    pub(super) fn published(&self) -> bool {
        self.published
    }

    fn root_check(&self) -> Result<()> {
        if self.published || self.cleaned {
            return Err("Zwischenordner ist nicht mehr offen".into());
        }
        for (path, expected) in &self.ancestors {
            let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || Stamp::read(&metadata).identity() != *expected
            {
                return Err("Zielordner oder ein Elternordner wurde ersetzt".into());
            }
        }
        if Stamp::read(&self.parent.metadata().map_err(|e| e.to_string())?).identity()
            != self.parent_identity
            || Stamp::read(&self.directory.metadata().map_err(|e| e.to_string())?).identity()
                != self.identity
            || anchored::child_stamp(&self.parent, OsStr::new(&self.stage_leaf))?.identity()
                != self.identity
        {
            return Err("Wiederherstellungs-Zwischenordner wurde ersetzt".into());
        }
        Ok(())
    }
    fn open_directory(&self, path: &str) -> Result<File> {
        let mut directory = self.directory.try_clone().map_err(|e| e.to_string())?;
        let mut prefix = String::new();
        if !path.is_empty() {
            for component in path.split('/') {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(component);
                let entry = self
                    .entries
                    .get(&prefix)
                    .ok_or("Ausgabe-Elternordner fehlt")?;
                if !entry.directory {
                    return Err("Ausgabe-Elternpfad ist keine Verzeichnisstruktur".into());
                }
                let stamp = anchored::child_stamp(&directory, OsStr::new(component))?;
                if !stamp.is_dir() || stamp.identity() != entry.identity {
                    return Err("Ausgabe-Elternordner wurde ersetzt".into());
                }
                directory = anchored::directory(&directory, OsStr::new(component))?;
                if Stamp::read(&directory.metadata().map_err(|e| e.to_string())?).identity()
                    != entry.identity
                {
                    return Err("Ausgabe-Elternordner wurde ersetzt".into());
                }
            }
        }
        Ok(directory)
    }
    fn reserve(&mut self, path: &str) -> Result<()> {
        if self.entries.len() + 1 >= self.max_entries {
            return Err("Ausgabe-Anzahllimit überschritten".into());
        }
        charge(&mut self.retained, self.max_retained, path.len() * 2 + 256)?;
        self.synced = false;
        Ok(())
    }
    fn record(&mut self, path: &str, file: &File, directory: bool) -> Result<()> {
        let stamp = match file.metadata() {
            Ok(metadata) => Stamp::read(&metadata),
            Err(error) => {
                self.unproven = true;
                self.control.mark_cleanup_failed();
                return Err(error.to_string());
            }
        };
        let parent = split(path).0;
        if parent.is_empty() {
            self.children += 1;
        } else {
            self.entries
                .get_mut(parent)
                .ok_or("Ausgabe-Elternordner fehlt")?
                .children += 1;
        }
        self.entries.insert(
            path.into(),
            Entry {
                identity: stamp.identity(),
                directory,
                children: 0,
                finished: None,
            },
        );
        Ok(())
    }
    pub(super) fn create_dir(&mut self, path: &str) -> Result<()> {
        relative(path)?;
        self.root_check()?;
        let mut prefix = String::new();
        for name in path.split('/') {
            check(self.control)?;
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(name);
            if let Some(entry) = self.entries.get(&prefix) {
                if !entry.directory {
                    return Err("Ausgabeordner kollidiert mit einer Datei".into());
                }
                self.open_directory(&prefix)?;
                continue;
            }
            self.reserve(&prefix)?;
            let parent = self.open_directory(split(&prefix).0)?;
            let mut retained = false;
            let result = output::create_at_report_retained(&parent, name, || {
                retained = true;
                self.control.mark_cleanup_failed();
            });
            self.unproven |= retained;
            let directory = result?;
            self.record(&prefix, &directory, true)?;
        }
        Ok(())
    }
    pub(super) fn create_file(&mut self, path: &str) -> Result<File> {
        check(self.control)?;
        relative(path)?;
        self.root_check()?;
        if self.entries.contains_key(path) {
            return Err("Ausgabedatei existiert bereits".into());
        }
        self.reserve(path)?;
        let (parent, name) = split(path);
        let parent = self.open_directory(parent)?;
        let file = output::create_file(&parent, name)?;
        self.record(path, &file, false)?;
        Ok(file)
    }
    pub(super) fn finish_file(&mut self, path: &str, file: &File) -> Result<()> {
        check(self.control)?;
        relative(path)?;
        self.root_check()?;
        let expected = self
            .entries
            .get(path)
            .ok_or("Ausgabedatei wurde nicht angelegt")?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if expected.directory
            || Stamp::read(&metadata).identity() != expected.identity
            || !metadata.is_file()
        {
            return Err("Ausgabedatei wurde ersetzt".into());
        }
        file.sync_all()
            .map_err(|e| format!("Ausgabedatei konnte nicht gespeichert werden: {e}"))?;
        let stamp = Stamp::read(&file.metadata().map_err(|e| e.to_string())?);
        let (parent, name) = split(path);
        let parent = self.open_directory(parent)?;
        if stamp.links() != 1 || anchored::child_stamp(&parent, OsStr::new(name))? != stamp {
            return Err("Ausgabedatei wurde während des Schreibens verändert".into());
        }
        self.entries
            .get_mut(path)
            .ok_or("Ausgabedatei fehlt")?
            .finished = Some(stamp);
        self.synced = false;
        Ok(())
    }
    fn closure(
        &self,
        path: &str,
        directory: &File,
        expected: usize,
        cancellable: bool,
    ) -> Result<()> {
        let listing = anchored::Listing::new(directory)?;
        let mut count = 0;
        while let Some(name) = listing.next()? {
            if cancellable {
                check(self.control)?;
            }
            let name = name.to_str().ok_or("Unbekannter Name im Zwischenordner")?;
            if name.len() > 255 {
                return Err("Unbekannter Name im Zwischenordner".into());
            }
            let child = if path.is_empty() {
                name.into()
            } else {
                format!("{path}/{name}")
            };
            if !self.entries.contains_key(&child) {
                return Err("Unbekannte Datei im Zwischenordner".into());
            }
            count += 1;
            if count > expected {
                return Err("Ausgabeordner wurde verändert".into());
            }
        }
        if count != expected {
            return Err("Ausgabedatei oder Ausgabeordner fehlt".into());
        }
        Ok(())
    }
    fn ownership(&self, finished: bool) -> Result<()> {
        self.root_check()?;
        if self.unproven {
            return Err("Ein Ausgabeordner konnte nicht sicher geöffnet werden".into());
        }
        self.closure("", &self.directory, self.children, finished)?;
        for (path, entry) in &self.entries {
            if finished {
                check(self.control)?;
            }
            let (parent, name) = split(path);
            let parent = self.open_directory(parent)?;
            let stamp = anchored::child_stamp(&parent, OsStr::new(name))?;
            if stamp.identity() != entry.identity
                || stamp.is_dir() != entry.directory
                || (!entry.directory && !stamp.is_file())
            {
                return Err("Ausgabedatei oder Ausgabeordner wurde ersetzt".into());
            }
            if entry.directory {
                let directory = self.open_directory(path)?;
                self.closure(path, &directory, entry.children, finished)?;
            } else if finished && entry.finished != Some(stamp) {
                return Err("Ausgabedatei wurde nicht gespeichert oder wurde verändert".into());
            }
        }
        Ok(())
    }
    pub(super) fn validate(&self) -> Result<()> {
        check(self.control)?;
        self.ownership(true)
    }
    pub(super) fn sync(&mut self) -> Result<()> {
        self.validate()?;
        for (path, entry) in self.entries.iter().rev() {
            if entry.directory {
                check(self.control)?;
                self.open_directory(path)?
                    .sync_all()
                    .map_err(|e| e.to_string())?;
            }
        }
        self.directory.sync_all().map_err(|e| e.to_string())?;
        self.synced = true;
        Ok(())
    }
    pub(super) fn publish(&mut self) -> Result<Option<String>> {
        self.validate()?;
        if !self.synced {
            return Err("Ausgabeordner wurde noch nicht gespeichert".into());
        }
        if anchored::optional_child_stamp(&self.parent, OsStr::new(&self.final_leaf))?.is_some() {
            return Err("Das Wiederherstellungsziel existiert bereits".into());
        }
        check(self.control)?;
        output::publish(&self.parent, &self.stage_leaf, OsStr::new(&self.final_leaf))?;
        self.published = true;
        Ok(self.parent.sync_all().err().map(|e| format!("Ausgabe wurde veröffentlicht, aber der Zielordner konnte nicht dauerhaft gespeichert werden: {e}")))
    }
    pub(super) fn cleanup(&mut self) -> bool {
        if self.published || self.cleaned {
            return true;
        }
        let complete = self.ownership(false).is_ok()
            && (|| {
                for (path, entry) in self.entries.iter().rev() {
                    let (parent, name) = split(path);
                    let Ok(parent) = self.open_directory(parent) else {
                        return false;
                    };
                    if !output::remove_owned(
                        &parent,
                        OsStr::new(name),
                        entry.identity,
                        entry.directory,
                    ) {
                        return false;
                    }
                }
                output::remove_owned(
                    &self.parent,
                    OsStr::new(&self.stage_leaf),
                    self.identity,
                    true,
                )
            })();
        self.cleaned = complete;
        if !complete {
            self.control.mark_cleanup_failed();
        }
        complete
    }
}
impl Drop for Stage<'_> {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::Stage;
    use crate::library::{unique_suffix, ImportControl};
    use std::{fs, io::Write, path::PathBuf};

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            let path = PathBuf::from("/private/tmp")
                .join(format!("game-assets-recovery-stage-{}", unique_suffix()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn recovery_stage_publishes_nested_files_and_empty_directories() {
        let workspace = Workspace::new();
        let control = ImportControl::default();
        let mut stage =
            Stage::create(&workspace.0, "restored-library", &control, 16, 65536).unwrap();
        stage.create_dir("library/sounds").unwrap();
        stage.create_dir("library/empty").unwrap();
        let mut output = stage.create_file("library/sounds/river.wav").unwrap();
        output.write_all(b"owned sound bytes").unwrap();
        stage
            .finish_file("library/sounds/river.wav", &output)
            .unwrap();
        drop(output);
        stage.validate().unwrap();
        stage.sync().unwrap();
        let temporary = stage.staged_root().to_path_buf();
        assert!(stage.publish().unwrap().is_none());
        assert!(stage.published());
        assert!(!temporary.exists());
        let final_root = workspace.0.join("restored-library");
        assert_eq!(
            fs::read(final_root.join("library/sounds/river.wav")).unwrap(),
            b"owned sound bytes"
        );
        assert!(final_root.join("library/empty").is_dir());
        assert!(stage.cleanup());
        assert!(final_root.exists());
    }
    fn write_output(stage: &mut Stage<'_>, path: &str, bytes: &[u8]) {
        let mut output = stage.create_file(path).unwrap();
        output.write_all(bytes).unwrap();
        stage.finish_file(path, &output).unwrap();
    }

    #[test]
    fn recovery_stage_stop_cleans_known_nested_partial_outputs() {
        let workspace = Workspace::new();
        let control = ImportControl::default();
        let mut stage = Stage::create(&workspace.0, "restored", &control, 16, 65536).unwrap();
        stage.create_dir("library/nested/empty").unwrap();
        let mut output = stage.create_file("library/nested/partial.wav").unwrap();
        output.write_all(b"partial owned bytes").unwrap();
        drop(output);
        let temporary = stage.staged_root().to_path_buf();
        control.cancel();
        assert!(stage.validate().is_err());
        assert!(stage.publish().is_err());
        assert!(stage.cleanup());
        assert!(!temporary.exists());
        assert!(!workspace.0.join("restored").exists());
        assert!(!control.cleanup_failed());
    }

    #[test]
    fn recovery_stage_refuses_unsafe_paths_limits_and_destination_collisions() {
        let workspace = Workspace::new();
        let control = ImportControl::default();
        assert!(Stage::create(&workspace.0, "restored", &control, 0, 65536).is_err());
        assert!(Stage::create(&workspace.0, "restored", &control, 16, 1).is_err());
        assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
        let mut stage = Stage::create(&workspace.0, "restored", &control, 2, 65536).unwrap();
        for invalid in [
            "",
            "/outside",
            ".",
            "..",
            "../outside",
            "a//b",
            "a/./b",
            "a/../b",
            "a\\b",
            "a\0b",
        ] {
            assert!(stage.create_dir(invalid).is_err(), "accepted {invalid:?}");
            assert!(stage.create_file(invalid).is_err(), "accepted {invalid:?}");
        }
        assert!(stage.create_dir(&"x".repeat(256)).is_err());
        assert!(stage.create_dir(&vec!["x"; 65].join("/")).is_err());
        assert_eq!(fs::read_dir(stage.staged_root()).unwrap().count(), 0);
        assert!(stage.create_dir("one/two").is_err());
        assert!(stage.staged_root().join("one").is_dir());
        assert!(!stage.staged_root().join("one/two").exists());
        assert!(stage.cleanup());
        drop(stage);
        control.cancel();
        assert!(Stage::create(&workspace.0, "cancelled", &control, 16, 65536).is_err());
        assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);

        let control = ImportControl::default();
        let destination = workspace.0.join("restored");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep.txt"), b"existing destination").unwrap();
        assert!(Stage::create(&workspace.0, "restored", &control, 16, 65536).is_err());
        assert_eq!(
            fs::read(destination.join("keep.txt")).unwrap(),
            b"existing destination"
        );
        fs::rename(&destination, workspace.0.join("prior-destination")).unwrap();
        let mut stage = Stage::create(&workspace.0, "restored", &control, 16, 65536).unwrap();
        write_output(&mut stage, "sound.wav", b"owned output");
        stage.sync().unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep.txt"), b"later collision").unwrap();
        assert!(stage.publish().is_err());
        assert!(!stage.published());
        assert!(stage.cleanup());
        assert_eq!(
            fs::read(destination.join("keep.txt")).unwrap(),
            b"later collision"
        );
        assert!(!control.cleanup_failed());
    }

    #[test]
    fn recovery_stage_retains_unknown_replaced_outputs_and_ancestors() {
        for replacement in ["file", "directory", "unknown", "stage", "ancestor"] {
            let workspace = Workspace::new();
            let destination = workspace.0.join("destination");
            fs::create_dir(&destination).unwrap();
            let control = ImportControl::default();
            let mut stage = Stage::create(&destination, "restored", &control, 16, 65536).unwrap();
            stage.create_dir("library/nested").unwrap();
            write_output(&mut stage, "library/nested/sound.wav", b"owned output");
            let root = stage.staged_root().to_path_buf();
            let preserved = match replacement {
                "file" => {
                    fs::rename(
                        root.join("library/nested/sound.wav"),
                        workspace.0.join("moved-owned.wav"),
                    )
                    .unwrap();
                    let p = root.join("library/nested/sound.wav");
                    fs::write(&p, b"unrelated replacement").unwrap();
                    p
                }
                "directory" => {
                    fs::rename(root.join("library"), workspace.0.join("moved-library")).unwrap();
                    fs::create_dir(root.join("library")).unwrap();
                    let p = root.join("library/keep.txt");
                    fs::write(&p, b"unrelated replacement").unwrap();
                    p
                }
                "unknown" => {
                    let p = root.join("keep.txt");
                    fs::write(&p, b"unrelated replacement").unwrap();
                    p
                }
                "stage" => {
                    fs::rename(&root, workspace.0.join("moved-stage")).unwrap();
                    fs::create_dir(&root).unwrap();
                    let p = root.join("keep.txt");
                    fs::write(&p, b"unrelated replacement").unwrap();
                    p
                }
                "ancestor" => {
                    fs::rename(&destination, workspace.0.join("moved-destination")).unwrap();
                    fs::create_dir(&destination).unwrap();
                    let p = destination.join("keep.txt");
                    fs::write(&p, b"unrelated replacement").unwrap();
                    p
                }
                _ => unreachable!(),
            };
            assert!(stage.validate().is_err(), "validated {replacement}");
            assert!(!stage.cleanup(), "removed {replacement}");
            assert!(
                control.cleanup_failed(),
                "lost retained evidence for {replacement}"
            );
            assert_eq!(fs::read(&preserved).unwrap(), b"unrelated replacement");
            assert!(!destination.join("restored").exists());
            drop(stage);
            assert_eq!(fs::read(&preserved).unwrap(), b"unrelated replacement");
        }
    }

    #[test]
    fn recovery_stage_refuses_unsealed_changed_outputs_and_keeps_committed_output_after_stop() {
        let workspace = Workspace::new();
        let control = ImportControl::default();
        let mut stage = Stage::create(&workspace.0, "unsealed", &control, 16, 65536).unwrap();
        let mut output = stage.create_file("sound.wav").unwrap();
        output.write_all(b"unfinished").unwrap();
        drop(output);
        assert!(stage.validate().is_err());
        assert!(stage.sync().is_err());
        assert!(stage.publish().is_err());
        assert!(stage.cleanup());
        drop(stage);
        let mut stage = Stage::create(&workspace.0, "changed", &control, 16, 65536).unwrap();
        write_output(&mut stage, "sound.wav", b"finished original");
        stage.sync().unwrap();
        fs::write(stage.staged_root().join("sound.wav"), b"changed").unwrap();
        assert!(stage.validate().is_err());
        assert!(stage.publish().is_err());
        assert!(stage.cleanup());
        drop(stage);
        let mut stage = Stage::create(&workspace.0, "committed", &control, 16, 65536).unwrap();
        write_output(&mut stage, "sound.wav", b"committed sound");
        stage.sync().unwrap();
        assert!(stage.publish().unwrap().is_none());
        control.cancel();
        assert!(stage.published());
        assert!(stage.cleanup());
        drop(stage);
        assert_eq!(
            fs::read(workspace.0.join("committed/sound.wav")).unwrap(),
            b"committed sound"
        );
        assert!(!control.cleanup_failed());
    }
}
