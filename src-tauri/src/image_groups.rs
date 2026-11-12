use super::{
    catalog_filter, path_text, AssetFile, AssetKind, CatalogPage, CatalogQuery, Library,
    ModelResources, Result, FILE_FOLDER,
};
use rusqlite::params;
use std::{
    collections::{hash_map::Entry, BTreeMap, HashMap, HashSet},
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageWorkflow {
    Material,
    SpriteSequence,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGroupSummary {
    pub workflow: ImageWorkflow,
    pub file_id: i64,
    pub name: String,
    pub file_count: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MaterialRole {
    BaseColor,
    Normal,
    Roughness,
    Metallic,
    Ao,
    Height,
}

impl MaterialRole {
    fn label(self) -> &'static str {
        match self {
            Self::BaseColor => "baseColor",
            Self::Normal => "normal",
            Self::Roughness => "roughness",
            Self::Metallic => "metallic",
            Self::Ao => "ao",
            Self::Height => "height",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGroupMember {
    pub asset_id: i64,
    pub file: AssetFile,
    pub role: Option<MaterialRole>,
    pub frame_index: Option<u64>,
    pub resource_path: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGroup {
    pub workflow: ImageWorkflow,
    pub name: String,
    pub members: Vec<ImageGroupMember>,
    pub resources: ModelResources,
    pub errors: Vec<String>,
}

struct ImageRecord {
    asset_id: i64,
    file_id: i64,
    package_id: i64,
    folder: String,
    filename: String,
}

pub(super) struct DerivedGroup {
    name: String,
    members: Vec<DerivedMember>,
}

struct DerivedMember {
    record: ImageRecord,
    role: Option<MaterialRole>,
    frame_index: Option<u64>,
}

/// Active catalog entries per sidebar type; each count matches the unfiltered catalog total for that type.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindCounts {
    pub sound: i64,
    pub image: i64,
    pub model: i64,
    pub animation: i64,
    pub material: i64,
    pub sprite_sequence: i64,
}

impl Library {
    pub fn kind_counts(&self) -> Result<KindCounts> {
        let kinds = [AssetKind::Sound, AssetKind::Image, AssetKind::Model];
        let animation = format!(
            "WHEN f.format IN ('{}') AND {} THEN 3",
            AssetKind::Animation.formats().join("','"),
            super::model_traits::animation_asset("f.asset_id")
        );
        let cases = std::iter::once(animation)
            .chain(kinds.iter().enumerate().map(|(index, kind)| {
                format!(
                    "WHEN f.format IN ('{}') THEN {index}",
                    kind.formats().join("','")
                )
            }))
            .collect::<Vec<_>>()
            .join(" ");
        let mut statement = self
            .connection
            .prepare(&format!("SELECT CASE {cases} END AS kind, COUNT(DISTINCT f.asset_id) FROM active_asset_files f GROUP BY kind"))
            .map_err(|error| format!("Typenzählung nicht lesbar: {error}"))?;
        let mut counts = KindCounts {
            material: self
                .derived_image_groups(ImageWorkflow::Material, None)?
                .len() as i64,
            sprite_sequence: self
                .derived_image_groups(ImageWorkflow::SpriteSequence, None)?
                .len() as i64,
            ..Default::default()
        };
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|error| format!("Typenzählung nicht lesbar: {error}"))?;
        for row in rows {
            match row.map_err(|error| format!("Typenzählung nicht lesbar: {error}"))? {
                (Some(0), count) => counts.sound = count,
                (Some(1), count) => counts.image = count,
                (Some(2), count) => counts.model = count,
                (Some(3), count) => counts.animation = count,
                _ => {}
            }
        }
        Ok(counts)
    }

    pub(super) fn query_image_groups(&self, query: CatalogQuery) -> Result<CatalogPage> {
        let workflow = query.workflow.ok_or("Bild-Arbeitsablauf fehlt")?;
        let mut groups = self.derived_image_groups(workflow, query.package_id)?;
        let (folder_filter, filter, text) = catalog_filter(&query);
        let mut statement = self
            .connection
            .prepare(&format!("{folder_filter} SELECT f.id {filter}"))
            .map_err(|error| format!("Katalogsuche fehlgeschlagen: {error}"))?;
        let matched = statement
            .query_map(
                params![
                    query.package_id,
                    text,
                    query.original_folder,
                    query.format,
                    query.tag,
                    query.folder_id,
                    query.asset_id
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| format!("Katalogsuche fehlgeschlagen: {error}"))?
            .collect::<std::result::Result<HashSet<_>, _>>()
            .map_err(|error| format!("Katalogsuche fehlgeschlagen: {error}"))?;
        groups.retain(|group| {
            group
                .members
                .iter()
                .any(|member| matched.contains(&member.record.file_id))
        });
        groups.sort_by_key(|group| {
            (
                group.name.to_ascii_lowercase(),
                group.members[0].record.package_id,
                group.members[0].record.folder.clone(),
                group.members[0].record.file_id,
            )
        });
        let total = groups.len() as i64;
        let offset = if query.offset >= total {
            0
        } else {
            query.offset.max(0)
        };
        let limit = if query.limit == 0 {
            40
        } else {
            query.limit.clamp(1, 100)
        };
        let page = groups
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect::<Vec<_>>();
        let mut assets = self.assets_by_id(
            &page
                .iter()
                .map(|group| group.members[0].record.asset_id)
                .collect::<Vec<_>>(),
        )?;
        self.hydrate(&mut assets)?;
        let mut items = Vec::new();
        for group in page {
            let representative = &group.members[0].record;
            let mut asset = assets
                .iter()
                .find(|asset| asset.id == representative.asset_id)
                .cloned()
                .ok_or("Material nicht im aktiven Katalog")?;
            asset
                .files
                .sort_by_key(|file| file.id != representative.file_id);
            asset.original_folder = representative.folder.clone();
            let errors = group.errors();
            asset.image_group = Some(ImageGroupSummary {
                workflow,
                file_id: representative.file_id,
                name: group.name,
                file_count: group.members.len(),
                errors,
            });
            items.push(asset);
        }
        Ok(CatalogPage {
            items,
            total,
            offset,
            limit,
        })
    }

    pub fn image_group(&self, file_id: i64, workflow: ImageWorkflow) -> Result<ImageGroup> {
        let (package_id, prefix): (i64, String) = self.connection.query_row(
            "SELECT p.id, p.relative_path FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id WHERE f.id = ?1",
            [file_id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).map_err(|error| format!("Bilddatei nicht im aktiven Katalog: {error}"))?;
        let group = self
            .derived_image_groups(workflow, Some(package_id))?
            .into_iter()
            .find(|group| {
                group
                    .members
                    .iter()
                    .any(|member| member.record.file_id == file_id)
            })
            .ok_or("Datei gehört zu keiner erkannten Bildgruppe")?;
        let errors = group.errors();
        let resources = self.image_resources(
            file_id,
            &group
                .members
                .iter()
                .map(|member| member.record.file_id)
                .collect::<Vec<_>>(),
        )?;
        let mut files = self.files_for_assets(
            &group
                .members
                .iter()
                .map(|member| member.record.asset_id)
                .collect::<Vec<_>>(),
        )?;
        let mut members = Vec::new();
        for member in group.members {
            let file = files
                .get_mut(&member.record.asset_id)
                .and_then(|files| {
                    let index = files
                        .iter()
                        .position(|file| file.id == member.record.file_id)?;
                    Some(files.swap_remove(index))
                })
                .ok_or("Bilddatei nicht im aktiven Katalog")?;
            let resource_path = path_text(
                Path::new(&file.relative_path)
                    .strip_prefix(&prefix)
                    .map_err(|_| "Bilddatei liegt außerhalb ihres Pakets")?,
            )?;
            members.push(ImageGroupMember {
                asset_id: member.record.asset_id,
                file,
                role: member.role,
                frame_index: member.frame_index,
                resource_path,
            });
        }
        Ok(ImageGroup {
            workflow,
            name: group.name,
            members,
            resources,
            errors,
        })
    }

    /// `derived` caches each package's groups so a bulk selection derives them once per package.
    pub(super) fn image_group_asset_ids(
        &self,
        file_id: i64,
        workflow: ImageWorkflow,
        derived: &mut HashMap<(ImageWorkflow, i64), Vec<DerivedGroup>>,
    ) -> Result<Vec<i64>> {
        let package_id: i64 = self.connection.query_row(
            "SELECT a.package_id FROM active_asset_files f JOIN assets a ON a.id = f.asset_id WHERE f.id = ?1",
            [file_id], |row| row.get(0),
        ).map_err(|error| format!("Bildgruppe nicht mehr im aktiven Katalog: {error}"))?;
        let groups = match derived.entry((workflow, package_id)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                entry.insert(self.derived_image_groups(workflow, Some(package_id))?)
            }
        };
        let group = groups
            .iter()
            .find(|group| {
                group
                    .members
                    .iter()
                    .any(|member| member.record.file_id == file_id)
            })
            .ok_or("Bildgruppe ist nicht mehr im aktiven Katalog. Es wurde nichts geändert.")?;
        Ok(group
            .members
            .iter()
            .map(|member| member.record.asset_id)
            .collect())
    }

    fn derived_image_groups(
        &self,
        workflow: ImageWorkflow,
        package_id: Option<i64>,
    ) -> Result<Vec<DerivedGroup>> {
        let mut statement = self.connection.prepare(&format!("SELECT f.asset_id, f.id, a.package_id, {FILE_FOLDER}, f.original_filename
            FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id
            WHERE (?1 IS NULL OR p.id = ?1) AND f.format IN ('{}') ORDER BY f.original_filename COLLATE NOCASE, f.id", AssetKind::Image.formats().join("','")))
            .map_err(|error| format!("Bildgruppen nicht lesbar: {error}"))?;
        let rows = statement
            .query_map([package_id], |row| {
                Ok(ImageRecord {
                    asset_id: row.get(0)?,
                    file_id: row.get(1)?,
                    package_id: row.get(2)?,
                    folder: row.get(3)?,
                    filename: row.get(4)?,
                })
            })
            .map_err(|error| format!("Bildgruppen nicht lesbar: {error}"))?;
        let mut groups = BTreeMap::<(i64, String, String), DerivedGroup>::new();
        for row in rows {
            let file = row.map_err(|error| format!("Bildgruppe nicht lesbar: {error}"))?;
            let recognized = match workflow {
                ImageWorkflow::Material => {
                    material_name(&file.filename).map(|(name, role)| (name, Some(role), None))
                }
                ImageWorkflow::SpriteSequence => {
                    sprite_name(&file.filename).map(|(name, index)| (name, None, Some(index)))
                }
            };
            let Some((name, role, frame_index)) = recognized else {
                continue;
            };
            let key = (
                file.package_id,
                file.folder.clone(),
                name.to_ascii_lowercase(),
            );
            groups
                .entry(key)
                .or_insert_with(|| DerivedGroup {
                    name,
                    members: Vec::new(),
                })
                .members
                .push(DerivedMember {
                    record: file,
                    role,
                    frame_index,
                });
        }
        let mut groups = groups.into_values().collect::<Vec<_>>();
        if workflow == ImageWorkflow::SpriteSequence {
            groups.retain(|group| group.members.len() > 1);
        }
        for group in &mut groups {
            group.members.sort_by_key(|member| {
                (
                    member.frame_index.unwrap_or_default(),
                    member.role != Some(MaterialRole::BaseColor),
                    member.record.filename.to_ascii_lowercase(),
                    member.record.file_id,
                )
            });
        }
        Ok(groups)
    }
}

fn sprite_name(filename: &str) -> Option<(String, u64)> {
    // Numbered source animations are independent timelines, not separate frame files.
    if Path::new(filename).extension().is_some_and(|extension| {
        ["gif", "ase", "aseprite"]
            .iter()
            .any(|format| extension.eq_ignore_ascii_case(format))
    }) {
        return None;
    }
    let stem = Path::new(filename).file_stem()?.to_str()?;
    let digits = stem.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits == stem.len() {
        return None;
    }
    let split = stem.len() - digits;
    let index = stem[split..].parse::<u64>().ok()?;
    // Frame indices cross the JSON boundary as exact JavaScript integers.
    if index > 9_007_199_254_740_991 {
        return None;
    }
    Some((stem[..split].into(), index))
}

impl DerivedGroup {
    fn errors(&self) -> Vec<String> {
        let mut counts = BTreeMap::<MaterialRole, usize>::new();
        let mut frames = BTreeMap::<u64, usize>::new();
        for member in &self.members {
            if let Some(role) = member.role {
                *counts.entry(role).or_default() += 1;
            }
            if let Some(index) = member.frame_index {
                *frames.entry(index).or_default() += 1;
            }
        }
        let mut errors = counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(role, count)| {
                format!(
                    "Materialrolle {} ist mehrdeutig ({count} Dateien). Bitte eine Map wählen.",
                    role.label()
                )
            })
            .collect::<Vec<_>>();
        errors.extend(frames.into_iter().filter(|(_, count)| *count > 1)
            .map(|(index, count)| format!("Frame-Index {index} ist mehrfach vorhanden ({count} Dateien). Die Sequenz ist nicht eindeutig.")));
        errors
    }
}

fn material_name(filename: &str) -> Option<(String, MaterialRole)> {
    let stem = Path::new(filename).file_stem()?.to_str()?;
    let lower = stem.to_ascii_lowercase();
    for (suffix, role) in [
        ("ambient_occlusion", MaterialRole::Ao),
        ("base_color", MaterialRole::BaseColor),
        ("basecolor", MaterialRole::BaseColor),
        ("roughness", MaterialRole::Roughness),
        ("metallic", MaterialRole::Metallic),
        ("metalness", MaterialRole::Metallic),
        ("normal", MaterialRole::Normal),
        ("height", MaterialRole::Height),
        ("albedo", MaterialRole::BaseColor),
        ("diffuse", MaterialRole::BaseColor),
        ("ao", MaterialRole::Ao),
    ] {
        if let Some(prefix) = lower.strip_suffix(suffix) {
            if prefix.ends_with(['_', '-', ' ']) {
                let name = stem[..prefix.len()].trim_end_matches(['_', '-', ' ']);
                if !name.is_empty() {
                    return Some((name.into(), role));
                }
            }
        }
    }
    None
}
