use super::{
    assets, modified_identity, path_text, unique_suffix, AssetKind, ExportOutcome, Library, Result,
};
use rusqlite::OptionalExtension;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

const MAX_RESOURCES: usize = 10_000;
// ponytail: bound manifest JSON to 16 MiB; stream it if large embedded-data models become common.
const MAX_MODEL_JSON: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelResource {
    pub relative_path: String,
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelResources {
    pub snapshot_id: String,
    pub file_id: i64,
    pub format: String,
    pub entry_path: String,
    pub resources: Vec<ModelResource>,
    pub declared_paths: Vec<String>,
    pub errors: Vec<String>,
    #[serde(skip)]
    library_root: PathBuf,
    #[serde(skip)]
    package_root: PathBuf,
    #[serde(skip)]
    package_prefix: String,
    #[serde(skip)]
    root_stamp: SourceStamp,
    #[serde(skip)]
    stamps: BTreeMap<String, SourceStamp>,
    #[serde(skip)]
    include_entry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceStamp {
    pub(super) identity: String,
    modified: String,
    size: u64,
}
impl SourceStamp {
    pub(super) fn read(metadata: &fs::Metadata) -> Result<Self> {
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            format!("{}:{}", metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let identity = format!(
            "{:?}",
            metadata
                .created()
                .map_err(|error| format!("Dateiidentität nicht lesbar: {error}"))?
        );
        Ok(Self {
            identity,
            modified: modified_identity(metadata)?,
            size: metadata.len(),
        })
    }
}

impl Library {
    pub fn model_resources(&self, file_id: i64) -> Result<ModelResources> {
        self.resource_snapshot(file_id, None)
    }

    pub(super) fn image_resources(&self, file_id: i64, members: &[i64]) -> Result<ModelResources> {
        self.resource_snapshot(file_id, Some(members))
    }

    pub(crate) fn sheet_resources(&self, file_id: i64) -> Result<ModelResources> {
        let snapshot = self.image_resources(file_id, &[file_id])?;
        self.validate_sheet_source(&snapshot)?;
        Ok(snapshot)
    }

    pub(super) fn validate_sheet_source(&self, snapshot: &ModelResources) -> Result<()> {
        if snapshot.library_root != self.root
            || snapshot.include_entry
            || snapshot.resources.len() != 1
            || !snapshot.errors.is_empty()
            || !snapshot.declared_paths.is_empty()
            || matches!(snapshot.format.as_str(), "GIF" | "ASEPRITE")
            || snapshot.resources[0].relative_path != snapshot.entry_path
        {
            return Err("Sprite-Export benötigt eine einzelne statische Bildvorschau".into());
        }
        self.resolve_preview(snapshot.file_id)?;
        let expected = snapshot
            .stamps
            .get(&snapshot.entry_path)
            .ok_or("Sprite-Quelle ist nicht vorbereitet")?;
        self.open_model_resource(snapshot, &snapshot.entry_path, expected)?;
        Ok(())
    }

    fn resource_snapshot(
        &self,
        file_id: i64,
        image_members: Option<&[i64]>,
    ) -> Result<ModelResources> {
        let entry = self.resolve_preview(file_id)?;
        let (format, prefix, external_root): (String, String, Option<String>) = self.connection.query_row(
            "SELECT f.format, p.relative_path, s.root FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id LEFT JOIN external_sources s ON s.package_id = p.id WHERE f.id = ?1",
            [file_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).map_err(|error| format!("Modelldatei nicht im aktiven Katalog: {error}"))?;
        let expected_kind = if image_members.is_some() {
            AssetKind::Image
        } else {
            AssetKind::Model
        };
        if AssetKind::for_format(&format) != Some(expected_kind) {
            return Err("Diese Asset-Datei passt nicht zum Vorschau-Arbeitsablauf".into());
        }
        let package_root = external_root
            .map(PathBuf::from)
            .unwrap_or_else(|| self.root.join(&prefix));
        let root_metadata = fs::symlink_metadata(&package_root)
            .map_err(|error| format!("Modellpaket nicht verfügbar: {error}"))?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err("Modellpaket wurde durch eine Verknüpfung ersetzt".into());
        }
        let entry_path = path_text(
            entry
                .strip_prefix(&package_root)
                .map_err(|_| "Modelldatei liegt außerhalb ihres Pakets")?,
        )?;
        let mut entry_input =
            fs::File::open(&entry).map_err(|error| format!("Modelldatei nicht lesbar: {error}"))?;
        let entry_stamp =
            SourceStamp::read(&entry_input.metadata().map_err(|error| error.to_string())?)?;
        let declared_paths = if matches!(format.as_str(), "GLTF" | "GLB") {
            gltf_paths(&mut entry_input, &entry_path, &format)?
        } else {
            Vec::new()
        };
        if SourceStamp::read(&entry_input.metadata().map_err(|error| error.to_string())?)?
            != entry_stamp
        {
            return Err("Modelldatei wurde während der Vorschau-Vorbereitung geändert".into());
        }
        let mut snapshot = ModelResources {
            snapshot_id: unique_suffix(),
            file_id,
            format,
            entry_path,
            resources: Vec::new(),
            declared_paths,
            errors: Vec::new(),
            library_root: self.root.clone(),
            package_root,
            package_prefix: prefix,
            root_stamp: SourceStamp::read(&root_metadata)?,
            stamps: BTreeMap::new(),
            include_entry: image_members.is_none(),
        };
        let mut candidates = BTreeSet::from([snapshot.entry_path.clone()]);
        if let Some(members) = image_members {
            if members.len() > MAX_RESOURCES {
                return Err("Bildgruppe enthält zu viele Dateien".into());
            }
            for member in members {
                let relative: String = self
                    .connection
                    .query_row(
                        "SELECT relative_path FROM active_asset_files WHERE id = ?1",
                        [member],
                        |row| row.get(0),
                    )
                    .map_err(|error| format!("Bilddatei nicht im aktiven Katalog: {error}"))?;
                candidates.insert(path_text(
                    Path::new(&relative)
                        .strip_prefix(&snapshot.package_prefix)
                        .map_err(|_| "Bilddatei liegt außerhalb ihres Pakets")?,
                )?);
            }
        } else if snapshot.format == "BLEND" {
            snapshot.errors.push("Blender: Nur gespeichertes Vorschaubild und Originaldatei; externe Texturen und verknüpfte Bibliotheken sind nicht aufgelöst.".into());
        } else {
            enumerate_resources(&snapshot.package_root, Path::new(""), &mut candidates)?;
        }
        candidates.extend(snapshot.declared_paths.iter().cloned());
        if candidates.len() > MAX_RESOURCES {
            return Err("Modellpaket enthält zu viele Vorschau-Ressourcen".into());
        }
        for relative in candidates {
            let resource = (|| {
                let path = self.model_source(&snapshot, &relative)?;
                let input = fs::File::open(&path)
                    .map_err(|error| format!("Modell-Ressource nicht lesbar: {error}"))?;
                let metadata = input.metadata().map_err(|error| error.to_string())?;
                if !metadata.is_file() {
                    return Err("Modell-Ressource ist keine reguläre Datei".into());
                }
                Ok((path, SourceStamp::read(&metadata)?))
            })();
            match resource {
                Ok((path, stamp)) => {
                    snapshot.resources.push(ModelResource {
                        relative_path: relative.clone(),
                        path: path_text(&path)?,
                        size_bytes: stamp.size,
                    });
                    if relative == snapshot.entry_path && stamp != entry_stamp {
                        return Err(
                            "Modelldatei wurde während der Vorschau-Vorbereitung ersetzt".into(),
                        );
                    }
                    snapshot.stamps.insert(relative, stamp);
                }
                Err(error) if relative == snapshot.entry_path => return Err(error),
                Err(error)
                    if image_members.is_some() || snapshot.declared_paths.contains(&relative) =>
                {
                    snapshot.errors.push(format!("{relative}: {error}"))
                }
                Err(_) => {}
            }
        }
        Ok(snapshot)
    }

    pub fn export_model(
        &self,
        snapshot: &ModelResources,
        resource_paths: &[String],
        destination: Option<&Path>,
    ) -> Result<ExportOutcome> {
        if snapshot.library_root != self.root {
            return Err("Die Modellvorschau gehört zu einer anderen Bibliothek".into());
        }
        self.resolve_preview(snapshot.file_id)?;
        if snapshot.format == "BLEND" {
            return Err("Blender kann nur als Originaldatei exportiert werden; externe Begleitdateien sind nicht aufgelöst".into());
        }
        if !snapshot.include_entry {
            let expected = snapshot
                .stamps
                .get(&snapshot.entry_path)
                .ok_or("Bildgruppe hat keine gültige repräsentative Datei")?;
            self.open_model_resource(snapshot, &snapshot.entry_path, expected)?;
        }
        if snapshot.include_entry && !snapshot.errors.is_empty() {
            return Err(format!(
                "Modell-Ressourcen fehlen: {}",
                snapshot.errors.join("; ")
            ));
        }
        let Some(destination) = destination else {
            return Ok(ExportOutcome::Cancelled);
        };
        if resource_paths.len() > MAX_RESOURCES {
            return Err("Zu viele Modell-Ressourcen für den Export".into());
        }
        let mut selected = BTreeSet::new();
        if snapshot.include_entry {
            selected.insert(snapshot.entry_path.clone());
        }
        selected.extend(snapshot.declared_paths.iter().cloned());
        selected.extend(resource_paths.iter().cloned());
        if selected.is_empty() {
            return Err("Bitte mindestens eine Bilddatei für den Export auswählen".into());
        }
        let mut inputs = Vec::new();
        for relative in selected {
            let expected = snapshot
                .stamps
                .get(&relative)
                .ok_or_else(|| format!("Ressource gehört nicht zur Modellvorschau: {relative}"))?;
            self.open_model_resource(snapshot, &relative, expected)?;
            inputs.push((relative, expected));
        }
        let parent = destination
            .parent()
            .ok_or("Exportziel hat keinen Ordner")?
            .canonicalize()
            .map_err(|error| format!("Exportordner nicht verfügbar: {error}"))?;
        self.validate_export_parent(&parent)?;
        let name = destination
            .file_name()
            .ok_or("Exportziel hat keinen Ordnernamen")?;
        let destination = parent.join(name);
        match fs::create_dir(&destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(
                    "Exportordner existiert bereits. Bitte einen neuen Ordnernamen wählen".into(),
                )
            }
            Err(error) => return Err(format!("Exportordner kann nicht erstellt werden: {error}")),
        }
        let mut owned_files = Vec::new();
        let directory_identity = SourceStamp::read(
            &fs::symlink_metadata(&destination).map_err(|error| error.to_string())?,
        )?
        .identity;
        let mut owned_dirs = vec![(destination.clone(), directory_identity)];
        let result = (|| {
            let mut size_bytes = 0_u64;
            for (relative, expected) in inputs {
                let mut input = self.open_model_resource(snapshot, &relative, expected)?;
                let target = destination.join(&relative);
                create_owned_dirs(
                    target.parent().ok_or("Exportziel hat keinen Ordner")?,
                    &destination,
                    &mut owned_dirs,
                )?;
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|error| {
                        format!("Modell-Ressource kann nicht exportiert werden: {error}")
                    })?;
                let stamp =
                    SourceStamp::read(&output.metadata().map_err(|error| error.to_string())?)?;
                owned_files.push((target.clone(), stamp.identity));
                let copied = io::copy(&mut input, &mut output).map_err(|error| {
                    format!("Modell-Ressource kann nicht kopiert werden: {error}")
                })?;
                size_bytes = size_bytes
                    .checked_add(copied)
                    .ok_or("Modellexport ist zu groß")?;
                output.sync_all().map_err(|error| {
                    format!("Modell-Ressource kann nicht gespeichert werden: {error}")
                })?;
                if SourceStamp::read(&input.metadata().map_err(|error| error.to_string())?)?
                    != *expected
                {
                    return Err(format!(
                        "Modell-Ressource wurde während des Exports verändert: {relative}"
                    ));
                }
                self.open_model_resource(snapshot, &relative, expected)?;
            }
            Ok(ExportOutcome::Exported {
                path: path_text(&destination)?,
                size_bytes,
            })
        })();
        if result.is_err() {
            for (path, identity) in owned_files.iter().rev() {
                if fs::symlink_metadata(path)
                    .ok()
                    .filter(|metadata| !metadata.file_type().is_symlink())
                    .and_then(|metadata| SourceStamp::read(&metadata).ok())
                    .is_some_and(|stamp| stamp.identity == *identity)
                {
                    let _ = fs::remove_file(path);
                }
            }
            for (path, identity) in owned_dirs.iter().rev() {
                if owned_directory(path, identity).is_ok() {
                    let _ = fs::remove_dir(path);
                }
            }
        }
        result
    }

    fn open_model_resource(
        &self,
        snapshot: &ModelResources,
        relative: &str,
        expected: &SourceStamp,
    ) -> Result<fs::File> {
        let path = self.model_source(snapshot, relative)?;
        let input = fs::File::open(&path)
            .map_err(|error| format!("Modell-Ressource nicht lesbar: {relative}: {error}"))?;
        let metadata = input.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || SourceStamp::read(&metadata)? != *expected {
            return Err(format!(
                "Modell-Ressource wurde geändert oder ersetzt: {relative}"
            ));
        }
        Ok(input)
    }

    fn model_source(&self, snapshot: &ModelResources, relative: &str) -> Result<PathBuf> {
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err("Ungültiger Modell-Ressourcenpfad".into());
        }
        let metadata = fs::symlink_metadata(&snapshot.package_root)
            .map_err(|error| format!("Modellpaket nicht verfügbar: {error}"))?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || SourceStamp::read(&metadata)?.identity != snapshot.root_stamp.identity
        {
            return Err("Modellpaket wurde geändert oder ersetzt".into());
        }
        let catalog_path = format!("{}/{}", snapshot.package_prefix, path_text(relative)?);
        let catalog: Option<(i64, bool)> = self.connection.query_row("SELECT f.id, EXISTS(SELECT 1 FROM active_asset_files active WHERE active.id = f.id) FROM asset_files f WHERE f.relative_path = ?1", [&catalog_path], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|error| format!("Modell-Ressourcen nicht lesbar: {error}"))?;
        if let Some((id, active)) = catalog {
            if !active {
                return Err("Modell-Ressource liegt im Papierkorb".into());
            }
            return self.resolve_preview(id);
        }
        let mut path = snapshot.package_root.clone();
        for component in relative.components() {
            path.push(component);
            if fs::symlink_metadata(&path)
                .map_err(|error| format!("Modell-Ressource nicht verfügbar: {error}"))?
                .file_type()
                .is_symlink()
            {
                return Err("Modell-Ressource wurde durch eine Verknüpfung ersetzt".into());
            }
        }
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("Modell-Ressource ist keine reguläre Datei".into());
        }
        // Filesystem case/Unicode aliases must retain catalog source and trash guards.
        if assets::candidate_format(relative).is_some() {
            let identity = SourceStamp::read(&metadata)?.identity;
            let mut statement = self.connection.prepare("SELECT f.id, f.relative_path, EXISTS(SELECT 1 FROM active_asset_files active WHERE active.id = f.id) FROM asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id WHERE p.relative_path = ?1")
                .map_err(|error| format!("Modell-Ressourcen nicht lesbar: {error}"))?;
            let catalog = statement
                .query_map([&snapshot.package_prefix], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                })
                .map_err(|error| format!("Modell-Ressourcen nicht lesbar: {error}"))?;
            for record in catalog {
                let (id, recorded, active) = record.map_err(|error| error.to_string())?;
                let recorded = Path::new(&recorded)
                    .strip_prefix(&snapshot.package_prefix)
                    .map_err(|_| "Ungültiger Modell-Ressourcenpfad im Katalog")?;
                if recorded
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                {
                    return Err("Ungültiger Modell-Ressourcenpfad im Katalog".into());
                }
                let recorded_path = snapshot.package_root.join(recorded);
                if fs::symlink_metadata(&recorded_path)
                    .ok()
                    .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
                    .and_then(|metadata| SourceStamp::read(&metadata).ok())
                    .is_some_and(|stamp| stamp.identity == identity)
                {
                    if !active {
                        return Err("Modell-Ressource liegt im Papierkorb".into());
                    }
                    return self.resolve_preview(id);
                }
            }
        }
        Ok(path)
    }
}

fn enumerate_resources(root: &Path, relative: &Path, paths: &mut BTreeSet<String>) -> Result<()> {
    let Ok(entries) = fs::read_dir(root.join(relative)) else {
        return Ok(());
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = relative.join(entry.file_name());
        if assets::is_macos_metadata(&path) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            enumerate_resources(root, &path, paths)?;
        } else if kind.is_file()
            && (assets::candidate_format(&path).and_then(AssetKind::for_format)
                == Some(AssetKind::Image)
                || path.extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("bin")
                        || extension.eq_ignore_ascii_case("mtl")
                        || extension.eq_ignore_ascii_case("tga")
                }))
        {
            if let Ok(path) = path_text(&path) {
                paths.insert(path);
            }
        }
        if paths.len() > MAX_RESOURCES {
            return Err("Modellpaket enthält zu viele Vorschau-Ressourcen".into());
        }
    }
    Ok(())
}

fn gltf_paths(input: &mut fs::File, entry_path: &str, format: &str) -> Result<Vec<String>> {
    let mut bytes = Vec::new();
    if format == "GLB" {
        let mut header = [0_u8; 20];
        input
            .read_exact(&mut header)
            .map_err(|error| format!("GLB-Header nicht lesbar: {error}"))?;
        let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let length = u32::from_le_bytes(header[8..12].try_into().unwrap());
        let json_length = u32::from_le_bytes(header[12..16].try_into().unwrap());
        let chunk_type = u32::from_le_bytes(header[16..20].try_into().unwrap());
        if &header[..4] != b"glTF"
            || version != 2
            || chunk_type != 0x4e4f534a
            || u64::from(length) != input.metadata().map_err(|error| error.to_string())?.len()
            || json_length > length.saturating_sub(20)
        {
            return Err("GLB-Modell benötigt einen gültigen glTF-2.0-JSON-Chunk".into());
        }
        if u64::from(json_length) > MAX_MODEL_JSON {
            return Err("Modell-JSON ist zu groß für die Vorschau".into());
        }
        bytes.resize(json_length as usize, 0);
        input
            .read_exact(&mut bytes)
            .map_err(|error| format!("GLB-JSON nicht lesbar: {error}"))?;
    } else {
        input
            .take(MAX_MODEL_JSON + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_MODEL_JSON {
            return Err("Modell-JSON ist zu groß für die Vorschau".into());
        }
    }
    let json: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("glTF-Modell nicht lesbar: {error}"))?;
    let mut paths = BTreeSet::new();
    for array in ["buffers", "images"] {
        if let Some(entries) = json[array].as_array() {
            for entry in entries {
                if let Some(uri) = entry["uri"].as_str() {
                    if !uri.starts_with("data:") {
                        paths.insert(normalize_reference(entry_path, uri)?);
                    }
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn normalize_reference(entry_path: &str, uri: &str) -> Result<String> {
    if uri.contains(['?', '#']) {
        return Err("Modell-Ressource enthält einen URL-Query oder ein Fragment".into());
    }
    let mut decoded = Vec::new();
    let bytes = uri.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = bytes
                .get(index + 1..index + 3)
                .ok_or("Ungültige URL-Kodierung im Modell")?;
            let pair =
                std::str::from_utf8(pair).map_err(|_| "Ungültige URL-Kodierung im Modell")?;
            decoded.push(
                u8::from_str_radix(pair, 16).map_err(|_| "Ungültige URL-Kodierung im Modell")?,
            );
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let uri = String::from_utf8(decoded)
        .map_err(|_| "Modell-Ressourcenpfad enthält ungültiges UTF-8")?
        .replace('\\', "/");
    if uri.is_empty() || uri.starts_with('/') || uri.contains([':', '\0']) {
        return Err("Modell-Ressource muss ein lokaler relativer Paketpfad sein".into());
    }
    let mut parts: Vec<&str> = entry_path
        .rsplit_once('/')
        .map_or(Vec::new(), |(parent, _)| parent.split('/').collect());
    for part in uri.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or("Modell-Ressource verlässt ihr Paket")?;
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err("Modell-Ressource hat keinen Dateipfad".into());
    }
    Ok(parts.join("/"))
}

fn owned_directory(path: &Path, identity: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Modell-Exportordner nicht verfügbar: {error}"))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || SourceStamp::read(&metadata)?.identity != identity
    {
        return Err("Modell-Exportordner wurde während des Exports ersetzt".into());
    }
    Ok(())
}

fn create_owned_dirs(
    parent: &Path,
    destination: &Path,
    created: &mut Vec<(PathBuf, String)>,
) -> Result<()> {
    let mut current = destination.to_owned();
    owned_directory(&current, &created[0].1)?;
    for component in parent
        .strip_prefix(destination)
        .map_err(|_| "Ungültiger Exportpfad")?
        .components()
    {
        current.push(component);
        if let Some((_, identity)) = created.iter().find(|(path, _)| path == &current) {
            owned_directory(&current, identity)?;
        } else {
            fs::create_dir(&current).map_err(|error| {
                format!("Modell-Exportordner kann nicht erstellt werden: {error}")
            })?;
            let identity = SourceStamp::read(
                &fs::symlink_metadata(&current).map_err(|error| error.to_string())?,
            )?
            .identity;
            created.push((current.clone(), identity));
        }
    }
    Ok(())
}
