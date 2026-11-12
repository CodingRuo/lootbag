use game_asset_library::library::{AssetKind, CatalogQuery, ExportOutcome, ImageWorkflow, Library};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "game-assets-aseprite-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn write(root: &Path, name: &str, bytes: &[u8]) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join(name), bytes).unwrap();
}

#[test]
fn source_animation_aliases_remain_images_variants_and_originals_without_numbered_sequence_grouping(
) {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    let inputs: [(&str, &[u8]); 5] = [
        ("Owned.ASE", b"owned ASE source"),
        ("Owned.aseprite", b"owned ASEPRITE variant"),
        ("Clip_01.ase", b"numbered source animation one"),
        ("Clip_02.ASEPRITE", b"numbered source animation two"),
        ("Rock_BaseColor.ase", b"source material map"),
    ];
    for (name, bytes) in inputs {
        write(&source, name, bytes);
    }
    let archive = workspace.path("source.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for (name, bytes) in inputs {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    let root = workspace.path("Library");
    let mut library = Library::create(&root).unwrap();
    let packages = [
        library.import_folder(&source).unwrap(),
        library.reference_folder(&source).unwrap(),
        library.import_zip(&archive).unwrap(),
    ];
    drop(library);
    let mut library = Library::open(&root).unwrap();
    for package in packages {
        let catalog = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                kind: Some(AssetKind::Image),
                format: Some("ASEPRITE".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(catalog.total, 4);
        let owned = catalog
            .items
            .iter()
            .find(|asset| asset.name == "Owned")
            .unwrap();
        assert_eq!(owned.files.len(), 2);
        assert!(library
            .catalog_filters(Some(package.id))
            .unwrap()
            .formats
            .iter()
            .any(|format| format == "ASEPRITE"));
        assert_eq!(
            library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    workflow: Some(ImageWorkflow::SpriteSequence),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
        let materials = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                workflow: Some(ImageWorkflow::Material),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(materials.total, 1);
        for asset in catalog.items {
            for file in asset.files {
                assert_eq!(file.kind, AssetKind::Image);
                assert_eq!(file.format, "ASEPRITE");
                let expected = inputs
                    .iter()
                    .find(|(name, _)| *name == file.original_filename)
                    .unwrap()
                    .1;
                assert_eq!(
                    fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
                    expected
                );
                let output =
                    workspace.path(&format!("export-{}-{}", package.id, file.original_filename));
                assert!(matches!(
                    library.export(file.id, Some(&output), false).unwrap(),
                    ExportOutcome::Exported { .. }
                ));
                assert_eq!(fs::read(output).unwrap(), expected);
                library.trash_file(file.id).unwrap();
                assert!(library.resolve_preview(file.id).is_err());
                assert!(library
                    .export(file.id, Some(&workspace.path("trashed.ase")), false)
                    .is_err());
            }
        }
    }
    assert!(!workspace.path("trashed.ase").exists());
    for (name, bytes) in inputs {
        assert_eq!(fs::read(source.join(name)).unwrap(), bytes);
    }
}

#[test]
fn source_maps_preserve_group_and_model_bundles_and_reject_changed_external_originals() {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    let image = b"owned ASE layered map original";
    let model = b"mtllib Surface.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Surface\nf 1 2 3\n";
    let material = b"newmtl Surface\nmap_Kd Rock_BaseColor.ase\n";
    for (name, bytes) in [
        ("Rock_BaseColor.ase", image.as_slice()),
        ("Surface.obj", model.as_slice()),
        ("Surface.mtl", material.as_slice()),
    ] {
        write(&source, name, bytes);
    }
    let root = workspace.path("Library");
    let mut library = Library::create(&root).unwrap();
    let packages = [
        library.import_folder(&source).unwrap(),
        library.reference_folder(&source).unwrap(),
    ];
    for package in packages {
        let image_id = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                kind: Some(AssetKind::Image),
                ..Default::default()
            })
            .unwrap()
            .items[0]
            .files[0]
            .id;
        let group = library
            .image_group(image_id, ImageWorkflow::Material)
            .unwrap();
        assert!(group.errors.is_empty());
        assert!(group.resources.errors.is_empty());
        let paths = group
            .members
            .iter()
            .map(|member| member.resource_path.clone())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["Rock_BaseColor.ase"]);
        let group_output = workspace.path(&format!("group-{}", package.id));
        library
            .export_model(&group.resources, &paths, Some(&group_output))
            .unwrap();
        assert_eq!(
            fs::read(group_output.join("Rock_BaseColor.ase")).unwrap(),
            image
        );
        let model_id = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                kind: Some(AssetKind::Model),
                ..Default::default()
            })
            .unwrap()
            .items[0]
            .files[0]
            .id;
        let snapshot = library.model_resources(model_id).unwrap();
        assert!(snapshot.errors.is_empty());
        assert_eq!(
            snapshot
                .resources
                .iter()
                .map(|resource| resource.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["Rock_BaseColor.ase", "Surface.mtl", "Surface.obj"]
        );
        let model_output = workspace.path(&format!("model-{}", package.id));
        let selected = vec!["Rock_BaseColor.ase".into(), "Surface.mtl".into()];
        library
            .export_model(&snapshot, &selected, Some(&model_output))
            .unwrap();
        for (name, expected) in [
            ("Rock_BaseColor.ase", image.as_slice()),
            ("Surface.obj", model.as_slice()),
            ("Surface.mtl", material.as_slice()),
        ] {
            assert_eq!(fs::read(model_output.join(name)).unwrap(), expected);
        }
        if package.source_kind == "external" {
            fs::write(source.join("Rock_BaseColor.ase"), b"changed source bytes").unwrap();
            assert!(library.resolve_preview(image_id).is_err());
            let original_output = workspace.path("stale-original.ase");
            assert!(library
                .export(image_id, Some(&original_output), false)
                .is_err());
            assert!(!original_output.exists());
            for (resources, selected, name) in [
                (&snapshot, selected, "stale-model"),
                (&group.resources, paths, "stale-group"),
            ] {
                let destination = workspace.path(name);
                assert!(library
                    .export_model(resources, &selected, Some(&destination))
                    .is_err());
                assert!(!destination.exists());
            }
        }
    }
}
