use super::{path_text, AssetFile, AssetKind, ExportOutcome, Library, ModelResources, Result};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs, io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_MEMBERS: usize = 10_000;
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectExportPlan {
    pub project: String,
    pub asset_count: usize,
    pub size_bytes: u64,
    /// "Name (Paket): Grund" for every member that would be skipped.
    pub missing: Vec<String>,
    /// Exported textures that replace a differently named model reference (see manifest notes).
    pub substitutions: usize,
}

struct Member {
    /// Subfolder path below the exported folder, "" or ending in "/".
    dir: String,
    asset_id: i64,
    name: String,
    role: Option<String>,
    package: String,
    package_prefix: String,
    file: Option<AssetFile>,
    /// Model snapshot plus extra package-relative resources to copy beside the entry.
    model: Option<(ModelResources, Vec<String>)>,
    missing: Option<String>,
    /// Texture substitutions the exported model still references by their original name.
    notes: Vec<String>,
    size_bytes: u64,
}

impl Library {
    fn project_members(&self, folder_id: i64) -> Result<(String, Vec<Member>)> {
        let project: String = self
            .connection
            .query_row(
                "SELECT name FROM folders WHERE id = ?1",
                [folder_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Projektordner nicht im Katalog: {e}"))?;
        let mut statement = self
            .connection
            .prepare(
                "WITH RECURSIVE tree(id, dir) AS (SELECT id, '' FROM folders WHERE id = ?1
                UNION ALL SELECT f.id, t.dir || f.name || '/' FROM folders f JOIN tree t ON f.parent_id = t.id)
            SELECT t.dir, a.id, a.name, fa.role, fa.file_id, p.name, p.relative_path FROM tree t
            JOIN folder_assets fa ON fa.folder_id = t.id JOIN active_assets a ON a.id = fa.asset_id
            JOIN packages p ON p.id = a.package_id ORDER BY t.dir, a.name COLLATE NOCASE, a.id",
            )
            .map_err(|e| format!("Projektordner nicht lesbar: {e}"))?;
        let rows = statement
            .query_map([folder_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|e| format!("Projektordner nicht lesbar: {e}"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| format!("Projektordner nicht lesbar: {e}"))?;
        if rows.len() > MAX_MEMBERS {
            return Err("Projektordner enthält zu viele Assets für einen Export".into());
        }
        let ids = rows.iter().map(|row| row.1).collect::<Vec<_>>();
        let files = self.files_for_assets(&ids)?;
        let mut members = Vec::new();
        for (dir, asset_id, name, role, file_id, package, package_prefix) in rows {
            let variants = files.get(&asset_id).map(Vec::as_slice).unwrap_or_default();
            // Trashed or reassigned variants are no longer active files of this asset: use the default.
            let file = file_id
                .and_then(|id| variants.iter().find(|file| file.id == id))
                .or(variants.first())
                .cloned();
            let mut member = Member {
                dir,
                asset_id,
                name,
                role,
                package,
                package_prefix,
                model: None,
                missing: None,
                notes: Vec::new(),
                size_bytes: file
                    .as_ref()
                    .map_or(0, |file| file.size_bytes.max(0) as u64),
                file,
            };
            match &member.file {
                None => member.missing = Some("keine aktive Variante".into()),
                Some(file) if !file.available => {
                    member.missing = Some("Datei nicht verfügbar".into())
                }
                Some(file) if file.kind == AssetKind::Model && file.format != "BLEND" => {
                    match self.model_resources(file.id) {
                        Ok(snapshot) if snapshot.errors.is_empty() => {
                            let extra = match companion_paths(&snapshot) {
                                Ok((extra, notes)) => {
                                    member.notes = notes;
                                    extra
                                }
                                Err(error) => {
                                    member.missing = Some(error);
                                    members.push(member);
                                    continue;
                                }
                            };
                            member.size_bytes = snapshot
                                .resources
                                .iter()
                                .filter(|resource| {
                                    resource.relative_path == snapshot.entry_path
                                        || snapshot.declared_paths.contains(&resource.relative_path)
                                        || extra.contains(&resource.relative_path)
                                })
                                .map(|resource| resource.size_bytes)
                                .sum();
                            member.model = Some((snapshot, extra));
                        }
                        Ok(snapshot) => member.missing = Some(snapshot.errors.join("; ")),
                        Err(error) => member.missing = Some(error),
                    }
                }
                Some(_) => {}
            }
            members.push(member);
        }
        Ok((project, members))
    }

    pub fn plan_project_export(&self, folder_id: i64) -> Result<ProjectExportPlan> {
        let (project, members) = self.project_members(folder_id)?;
        Ok(ProjectExportPlan {
            project,
            asset_count: members.len(),
            size_bytes: members
                .iter()
                .filter(|member| member.missing.is_none())
                .map(|member| member.size_bytes)
                .sum(),
            missing: members
                .iter()
                .filter_map(|member| {
                    let reason = member.missing.as_ref()?;
                    Some(format!("{} ({}): {reason}", member.name, member.package))
                })
                .collect(),
            substitutions: members
                .iter()
                .filter(|member| member.missing.is_none())
                .map(|member| member.notes.len())
                .sum(),
        })
    }

    /// Copies the folder's chosen variants into a new `<parent>/<folder name>` with `manifest.json`.
    /// Unavailable members are skipped and listed in the manifest; library originals are only read.
    /// A failed export keeps its partial folder: the user may have moved own files into it meanwhile.
    pub fn export_project(&self, folder_id: i64, parent: &Path) -> Result<ExportOutcome> {
        let (project, members) = self.project_members(folder_id)?;
        let parent = parent
            .canonicalize()
            .map_err(|e| format!("Exportordner nicht verfügbar: {e}"))?;
        self.validate_export_parent(&parent)?;
        let root = parent.join(&project);
        match fs::create_dir(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(
                    "Exportordner existiert bereits. Bitte einen anderen Zielordner wählen".into(),
                )
            }
            Err(error) => return Err(format!("Exportordner kann nicht erstellt werden: {error}")),
        }
        self.write_project(&root, &project, &members)
            .map_err(|error| format!("Export unvollständig: {}: {error}", root.display()))
    }

    fn write_project(
        &self,
        root: &Path,
        project: &str,
        members: &[Member],
    ) -> Result<ExportOutcome> {
        let mut used = HashSet::new();
        for member in members {
            let mut prefix = String::new();
            for part in member.dir.split_terminator('/') {
                prefix = format!("{prefix}{part}/");
                used.insert(prefix.trim_end_matches('/').to_lowercase());
            }
        }
        // Reserved before member files; only a subfolder named like it moves the manifest aside.
        let manifest_name = unique_name(&mut used, "", "manifest", ".json");
        let (mut assets, mut missing, mut size_bytes) = (Vec::new(), Vec::new(), 0_u64);
        for member in members {
            let file = match (&member.missing, &member.file) {
                (None, Some(file)) => file,
                (reason, _) => {
                    missing.push(json!({ "assetId": member.asset_id, "name": member.name, "package": member.package, "reason": reason }));
                    continue;
                }
            };
            let dir = root.join(&member.dir);
            fs::create_dir_all(&dir)
                .map_err(|e| format!("Exportordner kann nicht erstellt werden: {e}"))?;
            let filename = Path::new(&file.original_filename);
            let stem = filename
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("Asset");
            let (outcome, exported) = match &member.model {
                Some((snapshot, extra)) => {
                    let name = unique_name(&mut used, &member.dir, stem, "");
                    let outcome = self.export_model(snapshot, extra, Some(&dir.join(&name)))?;
                    let mut files = vec![snapshot.entry_path.clone()];
                    files.extend(snapshot.declared_paths.iter().chain(extra).cloned());
                    files.sort();
                    files.dedup();
                    let prefix = format!("{}{name}/", member.dir);
                    (
                        outcome,
                        files
                            .into_iter()
                            .map(|file| format!("{prefix}{file}"))
                            .collect::<Vec<_>>(),
                    )
                }
                None => {
                    let extension = filename
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .map_or(String::new(), |extension| format!(".{extension}"));
                    let name = unique_name(&mut used, &member.dir, stem, &extension);
                    let outcome = self.export(file.id, Some(&dir.join(&name)), false)?;
                    (outcome, vec![format!("{}{name}", member.dir)])
                }
            };
            let ExportOutcome::Exported {
                size_bytes: copied, ..
            } = outcome
            else {
                return Err(format!(
                    "Exportziel existiert bereits: {}",
                    exported.join(", ")
                ));
            };
            size_bytes += copied;
            assets.push(json!({
                "assetId": member.asset_id,
                "name": member.name,
                "role": member.role,
                "kind": file.kind,
                "package": member.package,
                "originalPath": file.relative_path.strip_prefix(&format!("{}/", member.package_prefix)).unwrap_or(&file.relative_path),
                "variant": { "fileId": file.id, "format": file.format, "file": file.original_filename },
                "exportedFiles": exported,
                "licenseNote": Value::Null,
                "notes": member.notes,
            }));
        }
        let manifest = json!({ "project": project, "exportedAt": iso_utc(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()), "assets": assets, "missing": missing });
        let bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(manifest_name))
            .map_err(|e| format!("Manifest kann nicht geschrieben werden: {e}"))?;
        io::Write::write_all(&mut output, &bytes)
            .and_then(|()| output.sync_all())
            .map_err(|e| format!("Manifest kann nicht geschrieben werden: {e}"))?;
        Ok(ExportOutcome::Exported {
            path: path_text(root)?,
            size_bytes,
        })
    }
}

/// Extra resources beyond the entry and its declared glTF references, plus a note for each
/// included file that stands in for a differently named reference (e.g. PNG for PSD).
// shortcut: FBX/OBJ are not parsed; a texture or .mtl is included when its file stem followed by
// "." appears in the model (or a matched .mtl), case-insensitively. Same-named files in other
// folders are included too, and PSD references also take same-stem PNG replacements.
fn companion_paths(snapshot: &ModelResources) -> Result<(Vec<String>, Vec<String>)> {
    if matches!(snapshot.format.as_str(), "GLTF" | "GLB") {
        return Ok(Default::default());
    }
    let read = |relative: &str| -> Result<Vec<u8>> {
        let resource = snapshot
            .resources
            .iter()
            .find(|resource| resource.relative_path == relative)
            .ok_or("Modell-Ressource fehlt")?;
        if resource.size_bytes > MAX_SCAN_BYTES {
            return Err(format!(
                "{relative}: zu groß für die Begleitdatei-Erkennung"
            ));
        }
        fs::read(&resource.path).map_err(|e| format!("{relative}: {e}"))
    };
    let stem = |path: &str| {
        Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .into_bytes()
    };
    // Raw texts keep the reference's spelling for notes; matching uses the lowercase copies.
    let mut texts = vec![read(&snapshot.entry_path)?];
    let mut lower = vec![texts[0].to_ascii_lowercase()];
    let mut names = mentioned_stems(&lower[0]);
    let others = snapshot
        .resources
        .iter()
        .map(|resource| resource.relative_path.as_str())
        .filter(|path| *path != snapshot.entry_path)
        .collect::<Vec<_>>();
    for mtl in others
        .iter()
        .filter(|path| path.to_ascii_lowercase().ends_with(".mtl"))
    {
        if names.contains(&stem(mtl)) {
            texts.push(read(mtl)?);
            lower.push(texts.last().unwrap().to_ascii_lowercase());
            names.extend(mentioned_stems(lower.last().unwrap()));
        }
    }
    let included = others
        .into_iter()
        .filter(|path| names.contains(&stem(path)))
        .collect::<Vec<_>>();
    let file_name = |path: &str| {
        Path::new(path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    let copied: HashSet<String> = included
        .iter()
        .map(|path| file_name(path).to_ascii_lowercase())
        .collect();
    // One pass per text: a texture reference whose exact file is not copied but whose stem is gets a note.
    let mut notes = Vec::new();
    for (index, raw) in texts.iter().enumerate() {
        for (stem_name, full, spelled, shorter) in texture_references(raw, index > 0) {
            if copied.contains(&full) || shorter.iter().any(|name| copied.contains(name)) {
                continue;
            }
            let Some(replacement) = included
                .iter()
                .find(|path| stem(path) == stem_name.as_bytes())
            else {
                continue;
            };
            let note = format!(
                "Textur {spelled} ersetzt durch {} – Referenz im Modell unverändert",
                file_name(replacement)
            );
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
    }
    Ok((included.into_iter().map(str::to_owned).collect(), notes))
}

const TEXTURE_EXTENSIONS: [&str; 12] = [
    "png", "jpg", "jpeg", "tga", "psd", "tif", "tiff", "bmp", "exr", "dds", "webp", "gif",
];

/// Texture file names referenced in a model or MTL text as (lowercase stem, lowercase name, original spelling,
/// lowercase shorter names after inner spaces). The whole name counts; an MTL line's leading keyword
/// ("map_Kd") is dropped. Shorter names only tell whether an ambiguous reference was copied anyway.
fn texture_references(text: &[u8], mtl: bool) -> Vec<(String, String, String, Vec<String>)> {
    let mut references = Vec::new();
    for (dot, _) in text.iter().enumerate().filter(|(_, byte)| **byte == b'.') {
        let extension = text[dot + 1..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric())
            .count();
        let ext = String::from_utf8_lossy(&text[dot + 1..dot + 1 + extension]).to_ascii_lowercase();
        if !TEXTURE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let floor = dot.saturating_sub(255);
        let separator = text[floor..dot].iter().rposition(|byte| {
            matches!(byte, b'/' | b'\\' | b'"' | b':') || byte.is_ascii_control()
        });
        let mut start = separator.map_or(floor, |position| floor + position + 1);
        let line_start = separator.is_none_or(|position| text[floor + position].is_ascii_control());
        if mtl && line_start {
            if let Some(space) = text[start..dot].iter().position(|byte| *byte == b' ') {
                start += space + 1;
            }
        }
        if start == dot {
            continue;
        }
        let end = dot + 1 + extension;
        let spelled = String::from_utf8_lossy(&text[start..end]).into_owned();
        let shorter = (start..dot)
            .filter(|at| text[*at] == b' ')
            .map(|at| String::from_utf8_lossy(&text[at + 1..end]).to_ascii_lowercase())
            .collect();
        let stem = String::from_utf8_lossy(&text[start..dot]).to_ascii_lowercase();
        references.push((stem, spelled.to_ascii_lowercase(), spelled, shorter));
    }
    references
}

/// Runs of name bytes directly before a ".", e.g. `texture_01_a` from `..\\textures\\texture_01_a.png`.
fn mentioned_stems(text: &[u8]) -> HashSet<Vec<u8>> {
    let mut stems = HashSet::new();
    for (dot, _) in text.iter().enumerate().filter(|(_, byte)| **byte == b'.') {
        let start = text[dot.saturating_sub(255)..dot]
            .iter()
            .rposition(|byte| matches!(byte, b'/' | b'\\' | b'"' | b':') || byte.is_ascii_control())
            .map_or(dot.saturating_sub(255), |position| {
                dot.saturating_sub(255) + position + 1
            });
        // OBJ/MTL put names after keywords ("map_Kd leaf.png"), so every suffix after a space counts too.
        for (offset, byte) in text[start..dot].iter().enumerate() {
            if offset == 0 || *byte == b' ' {
                stems.insert(text[start + offset + usize::from(*byte == b' ')..dot].to_vec());
            }
        }
    }
    stems
}

/// `stem.ext`, then `stem-2.ext`, … unique per export path, case-insensitively.
fn unique_name(used: &mut HashSet<String>, dir: &str, stem: &str, extension: &str) -> String {
    (1..)
        .map(|n| match n {
            1 => format!("{stem}{extension}"),
            n => format!("{stem}-{n}{extension}"),
        })
        .find(|name| used.insert(format!("{dir}{name}").to_lowercase()))
        .unwrap()
}

fn iso_utc(seconds: u64) -> String {
    let (days, time) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

#[cfg(test)]
#[path = "project_export_tests.rs"]
mod tests;
