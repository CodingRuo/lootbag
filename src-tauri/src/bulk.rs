use super::{normalized_tags, ImageWorkflow, Library, Result};
use rusqlite::params;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BulkSelection {
    Asset {
        asset_id: i64,
    },
    ImageGroup {
        file_id: i64,
        workflow: ImageWorkflow,
    },
}

#[derive(Debug, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BulkAction {
    AddTags { tags: Vec<String> },
    AddToFolder { folder_id: i64 },
    Trash,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkResult {
    pub asset_count: usize,
    pub file_count: i64,
}

impl Library {
    pub fn bulk_assets(
        &mut self,
        selection: &[BulkSelection],
        action: BulkAction,
    ) -> Result<BulkResult> {
        if selection.is_empty() || selection.len() > 100 {
            return Err(
                "Bitte 1 bis 100 Katalogeinträge auswählen. Es wurde nichts geändert.".into(),
            );
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| format!("Sammelaktion nicht verfügbar: {error}"))?;
        let mut ids = BTreeSet::new();
        let mut derived = HashMap::new();
        for item in selection {
            match item {
                BulkSelection::Asset { asset_id } => {
                    ids.insert(*asset_id);
                }
                BulkSelection::ImageGroup { file_id, workflow } => {
                    ids.extend(self.image_group_asset_ids(*file_id, *workflow, &mut derived)?)
                }
            }
            if ids.len() > 10_000 {
                return Err("Die Auswahl umfasst mehr als 10.000 Assets. Bitte kleinere Gruppen wählen. Es wurde nichts geändert.".into());
            }
        }
        let mut file_count = 0;
        for id in &ids {
            let count: i64 = transaction
                .query_row(
                    "SELECT count(*) FROM active_asset_files WHERE asset_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("Auswahl nicht lesbar: {error}"))?;
            if count == 0 {
                return Err(format!(
                    "Asset {id} ist nicht mehr im aktiven Katalog. Es wurde nichts geändert."
                ));
            }
            file_count += count;
        }
        match action {
            BulkAction::AddTags { tags } => {
                let tags = normalized_tags(&tags)?;
                if tags.is_empty() {
                    return Err("Mindestens einen Tag angeben. Es wurde nichts geändert.".into());
                }
                for id in &ids {
                    let mut statement = transaction
                        .prepare("SELECT tag FROM asset_tags WHERE asset_id = ?1")
                        .map_err(|error| format!("Tags nicht lesbar: {error}"))?;
                    let mut combined = statement
                        .query_map([id], |row| row.get::<_, String>(0))
                        .map_err(|error| format!("Tags nicht lesbar: {error}"))?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|error| format!("Tags nicht lesbar: {error}"))?;
                    combined.extend(tags.clone());
                    let combined = normalized_tags(&combined).map_err(|error| {
                        format!("Asset {id}: {error}. Es wurde nichts geändert.")
                    })?;
                    for tag in &combined {
                        transaction
                            .execute(
                                "INSERT OR IGNORE INTO asset_tags(asset_id, tag) VALUES (?1, ?2)",
                                params![id, tag],
                            )
                            .map_err(|error| format!("Tags nicht gespeichert: {error}"))?;
                    }
                }
            }
            BulkAction::AddToFolder { folder_id } => {
                let exists: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                        [folder_id],
                        |row| row.get(0),
                    )
                    .map_err(|error| format!("Projektordner nicht lesbar: {error}"))?;
                if !exists {
                    return Err(
                        "Projektordner ist nicht mehr vorhanden. Es wurde nichts geändert.".into(),
                    );
                }
                for id in &ids {
                    transaction.execute("INSERT OR IGNORE INTO folder_assets(folder_id, asset_id) VALUES (?1, ?2)", params![folder_id, id])
                        .map_err(|error| format!("Ordnerzuordnung nicht gespeichert: {error}"))?;
                }
            }
            BulkAction::Trash => {
                for id in &ids {
                    transaction.execute("INSERT INTO trash(file_id) SELECT id FROM active_asset_files WHERE asset_id = ?1", [id])
                        .map_err(|error| format!("Asset nicht in den Papierkorb verschoben: {error}"))?;
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("Sammelaktion nicht gespeichert: {error}"))?;
        Ok(BulkResult {
            asset_count: ids.len(),
            file_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::library::{BulkAction, BulkSelection, CatalogQuery, ImageWorkflow, Library};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "asset-bulk-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir_all(root.join("source")).unwrap();
            fs::write(root.join("source/river.wav"), b"original wav").unwrap();
            fs::write(root.join("source/river.ogg"), b"original ogg").unwrap();
            fs::write(root.join("source/tree.fbx"), b"original model").unwrap();
            Self(root)
        }
        fn library(&self) -> Library {
            let mut library = Library::create(&self.0.join("library")).unwrap();
            library.import_folder(&self.0.join("source")).unwrap();
            library
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn bulk_tags_add_to_mixed_assets_without_replacing_names_existing_tags_or_originals() {
        let workspace = Workspace::new();
        let mut library = workspace.library();
        let assets = library.query(CatalogQuery::default()).unwrap().items;
        let sound = assets.iter().find(|asset| asset.name == "river").unwrap();
        library
            .annotate_asset(sound.id, "River ambience", &["Water".into()])
            .unwrap();
        let selection = assets
            .iter()
            .map(|asset| BulkSelection::Asset { asset_id: asset.id })
            .collect::<Vec<_>>();
        let result = library
            .bulk_assets(
                &selection,
                BulkAction::AddTags {
                    tags: vec!["water".into(), "Salzburg".into()],
                },
            )
            .unwrap();
        assert_eq!(result.asset_count, 2);
        drop(library);
        let library = Library::open(&workspace.0.join("library")).unwrap();
        let assets = library.query(CatalogQuery::default()).unwrap().items;
        let sound = assets.iter().find(|asset| asset.id == sound.id).unwrap();
        assert_eq!(sound.name, "River ambience");
        assert_eq!(sound.tags, ["Salzburg", "Water"]);
        assert_eq!(sound.files.len(), 2);
        assert_eq!(
            assets
                .iter()
                .find(|asset| asset.name == "tree")
                .unwrap()
                .tags,
            ["Salzburg", "water"]
        );
        assert_eq!(
            fs::read(workspace.0.join("source/river.wav")).unwrap(),
            b"original wav"
        );
        assert_eq!(
            fs::read(library.resolve_preview(sound.files[0].id).unwrap()).unwrap(),
            b"original ogg"
        );
    }

    #[test]
    fn bulk_project_assignment_and_trash_recover_all_variants_and_memberships() {
        let workspace = Workspace::new();
        let mut library = workspace.library();
        let assets = library.query(CatalogQuery::default()).unwrap().items;
        let folder = library.create_folder("Salzburg Protocol", None).unwrap();
        let selection = assets
            .iter()
            .map(|asset| BulkSelection::Asset { asset_id: asset.id })
            .collect::<Vec<_>>();
        library
            .bulk_assets(
                &selection,
                BulkAction::AddToFolder {
                    folder_id: folder.id,
                },
            )
            .unwrap();
        library
            .bulk_assets(
                &selection,
                BulkAction::AddToFolder {
                    folder_id: folder.id,
                },
            )
            .unwrap();
        assert_eq!(
            library
                .query(CatalogQuery {
                    folder_id: Some(folder.id),
                    ..Default::default()
                })
                .unwrap()
                .total,
            2
        );
        let result = library.bulk_assets(&selection, BulkAction::Trash).unwrap();
        assert_eq!((result.asset_count, result.file_count), (2, 3));
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 0);
        let trash = library.trash_entries().unwrap();
        assert_eq!(trash.len(), 3);
        for entry in trash {
            library.restore_trash(entry.id).unwrap();
        }
        let recovered = library
            .query(CatalogQuery {
                folder_id: Some(folder.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(recovered.total, 2);
        assert_eq!(
            recovered
                .items
                .iter()
                .map(|asset| asset.files.len())
                .sum::<usize>(),
            3
        );
        for asset in recovered.items {
            for file in asset.files {
                assert_eq!(
                    fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
                    fs::read(workspace.0.join("source").join(file.original_filename)).unwrap()
                );
            }
        }
    }

    #[test]
    fn bulk_image_group_selection_includes_every_map_and_deduplicates_individual_selection() {
        let workspace = Workspace::new();
        fs::write(
            workspace.0.join("source/stone_basecolor.png"),
            b"base color",
        )
        .unwrap();
        fs::write(workspace.0.join("source/stone_normal.png"), b"normal map").unwrap();
        let mut library = workspace.library();
        let groups = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                ..Default::default()
            })
            .unwrap();
        let group = &groups.items[0];
        let result = library
            .bulk_assets(
                &[
                    BulkSelection::ImageGroup {
                        file_id: group.image_group.as_ref().unwrap().file_id,
                        workflow: ImageWorkflow::Material,
                    },
                    BulkSelection::Asset { asset_id: group.id },
                ],
                BulkAction::AddTags {
                    tags: vec!["Stone".into()],
                },
            )
            .unwrap();
        assert_eq!(result.asset_count, 2);
        let tagged = library
            .query(CatalogQuery {
                tag: Some("Stone".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(tagged.total, 2);
        assert!(tagged
            .items
            .iter()
            .all(|asset| asset.name.starts_with("stone_")));
    }

    #[test]
    fn bulk_failure_keeps_the_entire_selection_unchanged_when_one_asset_has_too_many_tags() {
        let workspace = Workspace::new();
        let mut library = workspace.library();
        let assets = library.query(CatalogQuery::default()).unwrap().items;
        let full = assets.iter().max_by_key(|asset| asset.id).unwrap();
        let tags = (1..=32)
            .map(|index| format!("existing-{index:02}"))
            .collect::<Vec<_>>();
        library.annotate_asset(full.id, &full.name, &tags).unwrap();
        let selection = assets
            .iter()
            .map(|asset| BulkSelection::Asset { asset_id: asset.id })
            .collect::<Vec<_>>();
        assert!(library
            .bulk_assets(
                &selection,
                BulkAction::AddTags {
                    tags: vec!["extra".into()]
                }
            )
            .unwrap_err()
            .contains("nichts geändert"));
        assert_eq!(
            library
                .query(CatalogQuery {
                    tag: Some("extra".into()),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            library
                .query(CatalogQuery {
                    asset_id: Some(full.id),
                    ..Default::default()
                })
                .unwrap()
                .items[0]
                .tags,
            tags
        );
        let missing = [
            BulkSelection::Asset {
                asset_id: assets[0].id,
            },
            BulkSelection::Asset { asset_id: i64::MAX },
        ];
        assert!(library.bulk_assets(&missing, BulkAction::Trash).is_err());
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 2);
        assert!(library.trash_entries().unwrap().is_empty());
    }

    #[test]
    fn bulk_groups_can_be_organized_and_recovered_while_an_external_source_is_offline() {
        let workspace = Workspace::new();
        let external = workspace.0.join("external");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("walk_01.png"), b"first frame").unwrap();
        fs::write(external.join("walk_02.png"), b"second frame").unwrap();
        let mut library = workspace.library();
        library.reference_folder(&external).unwrap();
        let group = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::SpriteSequence),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        let offline = workspace.0.join("offline");
        fs::rename(&external, &offline).unwrap();
        let selection = [BulkSelection::ImageGroup {
            file_id: group.image_group.unwrap().file_id,
            workflow: ImageWorkflow::SpriteSequence,
        }];
        let folder = library.create_folder("Animations", None).unwrap();
        library
            .bulk_assets(
                &selection,
                BulkAction::AddToFolder {
                    folder_id: folder.id,
                },
            )
            .unwrap();
        let result = library.bulk_assets(&selection, BulkAction::Trash).unwrap();
        assert_eq!((result.asset_count, result.file_count), (2, 2));
        for entry in library.trash_entries().unwrap() {
            library.restore_trash(entry.id).unwrap();
        }
        let recovered = library
            .query(CatalogQuery {
                folder_id: Some(folder.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(recovered.total, 2);
        assert!(recovered
            .items
            .iter()
            .all(|asset| !asset.files[0].available));
        assert_eq!(
            fs::read(offline.join("walk_01.png")).unwrap(),
            b"first frame"
        );
        assert_eq!(
            fs::read(offline.join("walk_02.png")).unwrap(),
            b"second frame"
        );
    }

    #[test]
    fn bulk_tags_keep_existing_spelling_when_unicode_case_differs() {
        let workspace = Workspace::new();
        let mut library = workspace.library();
        let asset = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        library
            .annotate_asset(asset.id, &asset.name, &["Ästhetik".into()])
            .unwrap();
        library
            .bulk_assets(
                &[BulkSelection::Asset { asset_id: asset.id }],
                BulkAction::AddTags {
                    tags: vec!["ästhetik".into()],
                },
            )
            .unwrap();
        assert_eq!(
            library
                .query(CatalogQuery {
                    asset_id: Some(asset.id),
                    ..Default::default()
                })
                .unwrap()
                .items[0]
                .tags,
            ["Ästhetik"]
        );
    }
}
