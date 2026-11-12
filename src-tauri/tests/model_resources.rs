use game_asset_library::library::{AssetKind, CatalogQuery, ExportOutcome, Library};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "game-assets-model-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
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

fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
fn model_id(library: &Library, name: &str) -> i64 {
    library
        .query(CatalogQuery {
            kind: Some(AssetKind::Model),
            ..Default::default()
        })
        .unwrap()
        .items
        .into_iter()
        .find(|asset| asset.name == name)
        .unwrap()
        .files[0]
        .id
}

#[test]
fn psd_and_tga_model_texture_resources_and_selected_bundles_preserve_managed_and_external_originals(
) {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    let psd = b"8BPS original layered photoshop texture";
    write(&source, "Models/Hero.fbx", b"original fbx source");
    write(&source, "Textures/Palette.PSD", psd);
    write(
        &source,
        "Textures/Misc/Wire_Alpha.TGA",
        b"original tga texture",
    );
    write(&source, "Textures/Unused.png", b"unused preview image");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    let packages = [
        library.import_folder(&source).unwrap(),
        library.reference_folder(&source).unwrap(),
    ];
    for package in packages {
        let psd_asset = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                kind: Some(AssetKind::Image),
                format: Some("PSD".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(psd_asset.total, 1);
        assert_eq!(psd_asset.items[0].files.len(), 1);
        assert_eq!(psd_asset.items[0].kind, AssetKind::Image);
        assert_eq!(psd_asset.items[0].files[0].format, "PSD");
        assert!(library
            .catalog_filters(Some(package.id))
            .unwrap()
            .formats
            .iter()
            .any(|format| format == "TGA"));
        let model = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                kind: Some(AssetKind::Model),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        let snapshot = library.model_resources(model.files[0].id).unwrap();
        assert!(snapshot.errors.is_empty());
        let texture = snapshot
            .resources
            .iter()
            .find(|resource| resource.relative_path == "Textures/Palette.PSD")
            .unwrap();
        assert_eq!(fs::read(&texture.path).unwrap(), psd);
        let tga = snapshot
            .resources
            .iter()
            .find(|resource| resource.relative_path == "Textures/Misc/Wire_Alpha.TGA")
            .unwrap();
        assert_eq!(fs::read(&tga.path).unwrap(), b"original tga texture");
        let destination = workspace.path(&format!("bundle-{}", package.id));
        library
            .export_model(
                &snapshot,
                &[
                    "Textures/Palette.PSD".into(),
                    "Textures/Misc/Wire_Alpha.TGA".into(),
                ],
                Some(&destination),
            )
            .unwrap();
        assert_eq!(
            fs::read(destination.join("Models/Hero.fbx")).unwrap(),
            b"original fbx source"
        );
        assert_eq!(
            fs::read(destination.join("Textures/Palette.PSD")).unwrap(),
            psd
        );
        assert_eq!(
            fs::read(destination.join("Textures/Misc/Wire_Alpha.TGA")).unwrap(),
            b"original tga texture"
        );
        assert!(!destination.join("Textures/Unused.png").exists());
        assert_eq!(fs::read(source.join("Textures/Palette.PSD")).unwrap(), psd);
        assert_eq!(
            fs::read(source.join("Textures/Misc/Wire_Alpha.TGA")).unwrap(),
            b"original tga texture"
        );
        assert!(library
            .export_model(
                &snapshot,
                &["Textures/Palette.PSD".into()],
                Some(&destination)
            )
            .is_err());
        assert_eq!(
            fs::read(destination.join("Textures/Palette.PSD")).unwrap(),
            psd
        );
    }
}

#[test]
fn model_manifest_and_bundle_keep_only_selected_dependencies_and_original_paths() {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    let model = br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"../Buffers/triangle.payload","byteLength":3}],"images":[{"uri":"../Textures/Checker%20Board.png"},{"uri":"data:image/png;base64,AA=="}]}"#;
    write(&source, "Models/Triangle.gltf", model);
    write(&source, "Buffers/triangle.payload", b"123");
    write(&source, "Textures/Checker Board.png", b"PNG source");
    write(&source, "Unrelated.mp3", b"audio source");
    write(&source, "Models/Other.obj", b"unrelated model");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&source).unwrap();
    let id = model_id(&library, "Triangle");
    let snapshot = library.model_resources(id).unwrap();
    assert_eq!(snapshot.file_id, id);
    assert_eq!(snapshot.entry_path, "Models/Triangle.gltf");
    assert_eq!(
        snapshot.declared_paths,
        ["Buffers/triangle.payload", "Textures/Checker Board.png"]
    );
    assert!(snapshot.errors.is_empty());
    assert!(!snapshot
        .resources
        .iter()
        .any(|resource| resource.relative_path == "Unrelated.mp3"
            || resource.relative_path == "Models/Other.obj"));
    let destination = workspace.path("Game/Models/Triangle");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let outcome = library
        .export_model(&snapshot, &[], Some(&destination))
        .unwrap();
    assert!(matches!(outcome, ExportOutcome::Exported { .. }));
    for (relative, expected) in [
        ("Models/Triangle.gltf", model.as_slice()),
        ("Buffers/triangle.payload", b"123".as_slice()),
        ("Textures/Checker Board.png", b"PNG source".as_slice()),
    ] {
        assert_eq!(fs::read(destination.join(relative)).unwrap(), expected);
        assert_eq!(fs::read(source.join(relative)).unwrap(), expected);
        let resource = snapshot
            .resources
            .iter()
            .find(|resource| resource.relative_path == relative)
            .unwrap();
        assert_eq!(fs::read(&resource.path).unwrap(), expected);
    }
    assert!(!destination.join("Unrelated.mp3").exists());
    assert!(!destination.join("Models/Other.obj").exists());
}

#[test]
fn glb_manifest_reads_declared_resources_without_reading_the_binary_chunk() {
    use std::io::Write;
    let workspace = Workspace::new();
    let source = workspace.path("source");
    fs::create_dir_all(source.join("Models")).unwrap();
    let mut json = br#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":17825792}],"images":[{"uri":"../Textures/unused.png"}]}"#.to_vec();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let binary_length = 17 * 1024 * 1024_u32;
    let total = 12 + 8 + json.len() as u32 + 8 + binary_length;
    let mut file = fs::File::create(source.join("Models/Large.glb")).unwrap();
    file.write_all(b"glTF").unwrap();
    file.write_all(&2_u32.to_le_bytes()).unwrap();
    file.write_all(&total.to_le_bytes()).unwrap();
    file.write_all(&(json.len() as u32).to_le_bytes()).unwrap();
    file.write_all(&0x4e4f534a_u32.to_le_bytes()).unwrap();
    file.write_all(&json).unwrap();
    file.write_all(&binary_length.to_le_bytes()).unwrap();
    file.write_all(&0x004e4942_u32.to_le_bytes()).unwrap();
    file.set_len(total.into()).unwrap();
    drop(file);
    write(&source, "Textures/unused.png", b"unused but declared image");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.reference_folder(&source).unwrap();
    let snapshot = library
        .model_resources(model_id(&library, "Large"))
        .unwrap();
    assert_eq!(snapshot.declared_paths, ["Textures/unused.png"]);
    assert_eq!(snapshot.resources.len(), 2);
    assert!(snapshot.errors.is_empty());
}

#[test]
fn missing_trashed_changed_and_unrelated_model_resources_never_publish_a_bundle() {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    write(&source, "Models/Triangle.gltf", br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"../Buffers/triangle.bin","byteLength":3}],"images":[{"uri":"../Textures/Checker.png"}]}"#);
    write(&source, "Buffers/triangle.bin", b"123");
    write(&source, "Textures/Checker.png", b"image original");
    write(&source, "Unrelated.mp3", b"unrelated audio");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    let package = library.import_folder(&source).unwrap();
    let id = model_id(&library, "Triangle");
    let original = library.model_resources(id).unwrap();
    let output = workspace.path("bundle");
    for relative in [
        "../outside.png",
        "/etc/passwd",
        "https://example.com/texture.png",
        "Unrelated.mp3",
    ] {
        assert!(library
            .export_model(&original, &[relative.into()], Some(&output))
            .is_err());
        assert!(!output.exists());
    }
    let buffer = original
        .resources
        .iter()
        .find(|resource| resource.relative_path == "Buffers/triangle.bin")
        .unwrap();
    fs::write(&buffer.path, b"changed buffer").unwrap();
    let error = library
        .export_model(&original, &[], Some(&output))
        .unwrap_err();
    assert!(
        error.contains("geändert") || error.contains("ersetzt"),
        "{error}"
    );
    assert!(!output.exists());
    fs::write(&buffer.path, b"123").unwrap();
    let fresh = library.model_resources(id).unwrap();
    fs::remove_file(&buffer.path).unwrap();
    let missing_buffer = library.model_resources(id).unwrap();
    assert_eq!(missing_buffer.errors.len(), 1);
    assert!(missing_buffer.errors[0].contains("Buffers/triangle.bin"));
    assert!(library.export_model(&fresh, &[], Some(&output)).is_err());
    assert!(library
        .export_model(&missing_buffer, &[], Some(&output))
        .is_err());
    assert!(!output.exists());
    fs::write(&buffer.path, b"123").unwrap();
    let fresh = library.model_resources(id).unwrap();
    let texture = library
        .query(CatalogQuery {
            package_id: Some(package.id),
            kind: Some(AssetKind::Image),
            ..Default::default()
        })
        .unwrap()
        .items
        .remove(0)
        .files[0]
        .id;
    library.trash_file(texture).unwrap();
    assert!(library.export_model(&fresh, &[], Some(&output)).is_err());
    assert!(!output.exists());
    let missing = library.model_resources(id).unwrap();
    assert_eq!(missing.errors.len(), 1);
    assert!(missing.errors[0].contains("Papierkorb"));
    assert!(library.export_model(&missing, &[], Some(&output)).is_err());
    assert!(!output.exists());
}

#[test]
fn bundle_export_preserves_existing_project_folders_and_protected_sources() {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    write(
        &source,
        "Triangle.obj",
        b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
    );
    write(&source, "Material.mtl", b"newmtl Surface\nKd 1 0 0\n");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.reference_folder(&source).unwrap();
    let id = model_id(&library, "Triangle");
    let snapshot = library.model_resources(id).unwrap();
    for name in ["existing-empty", "existing-project"] {
        let destination = workspace.path(name);
        fs::create_dir(&destination).unwrap();
        if name == "existing-project" {
            fs::write(destination.join("game.scene"), b"project work").unwrap();
        }
        assert!(library
            .export_model(&snapshot, &["Material.mtl".into()], Some(&destination))
            .is_err());
        assert!(destination.is_dir());
        if name == "existing-project" {
            assert_eq!(
                fs::read(destination.join("game.scene")).unwrap(),
                b"project work"
            );
        }
    }
    for destination in [source.join("export"), library.root().join("export")] {
        assert!(library
            .export_model(&snapshot, &[], Some(&destination))
            .is_err());
        assert!(!destination.exists());
    }
    let destination = workspace.path("new-bundle");
    library
        .export_model(&snapshot, &["Material.mtl".into()], Some(&destination))
        .unwrap();
    assert_eq!(
        fs::read(destination.join("Triangle.obj")).unwrap(),
        b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"
    );
    assert_eq!(
        fs::read(destination.join("Material.mtl")).unwrap(),
        b"newmtl Surface\nKd 1 0 0\n"
    );
    assert_eq!(
        library.export_model(&snapshot, &[], None).unwrap(),
        ExportOutcome::Cancelled
    );
}

#[test]
fn declared_model_paths_reject_network_absolute_and_package_escaping_references() {
    let workspace = Workspace::new();
    for (index, uri) in [
        "../../outside.bin",
        "https://example.com/buffer.bin",
        "file:///etc/passwd",
        "//example.com/buffer.bin",
        "C:\\buffer.bin",
        "%2fetc%2fpasswd",
        "../bad%00.bin",
        "../bad%gg.bin",
    ]
    .into_iter()
    .enumerate()
    {
        let source = workspace.path(&format!("source-{index}"));
        let model = format!(
            "{{\"asset\":{{\"version\":\"2.0\"}},\"buffers\":[{{\"uri\":{},\"byteLength\":3}}]}}",
            serde_json::to_string(uri).unwrap()
        );
        write(&source, "Models/Triangle.gltf", model.as_bytes());
        let mut library = Library::create(&workspace.path(&format!("library-{index}"))).unwrap();
        library.import_folder(&source).unwrap();
        assert!(
            library
                .model_resources(model_id(&library, "Triangle"))
                .is_err(),
            "{uri}"
        );
        assert_eq!(
            fs::read(source.join("Models/Triangle.gltf")).unwrap(),
            model.as_bytes()
        );
    }
}

#[test]
#[cfg(unix)]
fn unavailable_unrelated_texture_does_not_block_the_selected_model() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = Workspace::new();
    let source = workspace.path("source");
    write(
        &source,
        "Models/Triangle.gltf",
        br#"{"asset":{"version":"2.0"}}"#,
    );
    write(&source, "Unrelated/Unused.png", b"unrelated image");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&source).unwrap();
    let image = library
        .query(CatalogQuery {
            kind: Some(AssetKind::Image),
            ..Default::default()
        })
        .unwrap()
        .items
        .remove(0)
        .files[0]
        .id;
    let image_path = library.resolve_preview(image).unwrap();
    fs::set_permissions(&image_path, fs::Permissions::from_mode(0o0)).unwrap();
    let result = library.model_resources(model_id(&library, "Triangle"));
    fs::set_permissions(&image_path, fs::Permissions::from_mode(0o600)).unwrap();
    let snapshot = result.expect("an unreadable unrelated image must not prevent model preview");
    assert!(snapshot.errors.is_empty());
    assert_eq!(snapshot.resources.len(), 1);
    let parent = image_path.parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o0)).unwrap();
    let result = library.model_resources(model_id(&library, "Triangle"));
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.unwrap().errors.is_empty());
}

#[test]
#[cfg(unix)]
fn substituted_external_roots_files_and_symlinked_dependencies_are_unavailable() {
    use std::os::unix::fs::symlink;
    let workspace = Workspace::new();
    let source = workspace.path("source");
    write(&source, "Models/Triangle.gltf", br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"../Buffers/triangle.bin","byteLength":3}],"images":[{"uri":"../Textures/Checker.png"}]}"#);
    write(&source, "Buffers/triangle.bin", b"123");
    write(&source, "Textures/Checker.png", b"image original");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.reference_folder(&source).unwrap();
    let id = model_id(&library, "Triangle");
    let snapshot = library.model_resources(id).unwrap();
    let output = workspace.path("bundle");
    for relative in [
        "Models/Triangle.gltf",
        "Textures/Checker.png",
        "Buffers/triangle.bin",
    ] {
        let path = source.join(relative);
        let saved = path.with_extension("saved");
        let original = fs::read(&path).unwrap();
        fs::rename(&path, &saved).unwrap();
        fs::write(&path, &original).unwrap();
        assert!(
            library.export_model(&snapshot, &[], Some(&output)).is_err(),
            "substitution: {relative}"
        );
        assert!(!output.exists());
        fs::remove_file(&path).unwrap();
        fs::rename(&saved, &path).unwrap();
    }
    let buffer = source.join("Buffers/triangle.bin");
    let saved = source.join("Buffers/original.saved");
    fs::rename(&buffer, &saved).unwrap();
    write(&workspace.0, "outside.bin", b"123");
    symlink(workspace.path("outside.bin"), &buffer).unwrap();
    let symlinked = library.model_resources(id).unwrap();
    assert_eq!(symlinked.errors.len(), 1);
    assert!(symlinked.errors[0].contains("Verknüpfung"));
    assert!(library.export_model(&snapshot, &[], Some(&output)).is_err());
    assert!(!output.exists());
    fs::remove_file(&buffer).unwrap();
    fs::rename(saved, buffer).unwrap();
    let original_root = workspace.path("original-source");
    fs::rename(&source, &original_root).unwrap();
    write(&source, "Models/Triangle.gltf", b"substituted root");
    assert!(library.model_resources(id).is_err());
    assert!(library.export_model(&snapshot, &[], Some(&output)).is_err());
    assert!(!output.exists());
    fs::remove_dir_all(&source).unwrap();
    fs::rename(original_root, source).unwrap();
    library.trash_file(id).unwrap();
    assert!(library.model_resources(id).is_err());
    assert!(library.export_model(&snapshot, &[], Some(&output)).is_err());
    assert!(!output.exists());
}

#[test]
#[cfg(unix)]
fn case_insensitive_texture_aliases_preserve_catalog_trash_guards() {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    write(
        &source,
        "Models/Triangle.gltf",
        br#"{"asset":{"version":"2.0"},"images":[{"uri":"../Textures/checker.png"}]}"#,
    );
    write(&source, "Textures/Checker.png", b"image original");
    if !source.join("Textures/checker.png").exists() {
        return;
    }
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&source).unwrap();
    let id = model_id(&library, "Triangle");
    let texture = library
        .query(CatalogQuery {
            kind: Some(AssetKind::Image),
            ..Default::default()
        })
        .unwrap()
        .items
        .remove(0)
        .files[0]
        .id;
    let snapshot = library.model_resources(id).unwrap();
    assert!(snapshot.errors.is_empty());
    library.trash_file(texture).unwrap();
    let hidden = library.model_resources(id).unwrap();
    assert_eq!(hidden.errors.len(), 1);
    assert!(hidden.errors[0].contains("Papierkorb"));
    let output = workspace.path("bundle");
    assert!(library.export_model(&snapshot, &[], Some(&output)).is_err());
    assert!(!output.exists());
}

#[test]
#[cfg(unix)]
fn encoded_texture_filename_punctuation_is_a_local_file_not_a_url_fragment() {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    write(
        &source,
        "Triangle.gltf",
        br#"{"asset":{"version":"2.0"},"images":[{"uri":"Textures/Checker%20%231%3F.png"}]}"#,
    );
    write(&source, "Textures/Checker #1?.png", b"image source");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&source).unwrap();
    let snapshot = library
        .model_resources(model_id(&library, "Triangle"))
        .unwrap();
    assert_eq!(snapshot.declared_paths, ["Textures/Checker #1?.png"]);
    assert!(snapshot.errors.is_empty());
    let output = workspace.path("bundle");
    library.export_model(&snapshot, &[], Some(&output)).unwrap();
    assert_eq!(
        fs::read(output.join("Textures/Checker #1?.png")).unwrap(),
        b"image source"
    );
}

#[test]
fn blender_imports_prepare_only_guarded_entry_and_export_only_original_bytes() {
    use std::io::Write;
    let workspace = Workspace::new();
    let source = workspace.path("source");
    let original = b"BLENDER17-01v0502 owned Blender source";
    write(&source, "Scenes/Owned.BLEND", original);
    write(
        &source,
        "Textures/Companion.png",
        b"external companion not resolved",
    );
    let archive = workspace.path("blender.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    zip.start_file(
        "Scenes/Owned.BLEND",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(original).unwrap();
    zip.finish().unwrap();
    let root = workspace.path("library");
    let mut library = Library::create(&root).unwrap();
    let packages = [
        library.import_folder(&source).unwrap(),
        library.reference_folder(&source).unwrap(),
        library.import_zip(&archive).unwrap(),
    ];
    drop(library);
    let mut library = Library::open(&root).unwrap();
    for package in packages {
        let result = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                kind: Some(AssetKind::Model),
                format: Some("BLEND".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.total, 1);
        let file = &result.items[0].files[0];
        assert_eq!(file.original_filename, "Owned.BLEND");
        assert_eq!(file.format, "BLEND");
        let mut snapshot = library.model_resources(file.id).unwrap();
        assert_eq!(snapshot.entry_path, "Scenes/Owned.BLEND");
        assert_eq!(snapshot.resources.len(), 1);
        assert_eq!(snapshot.resources[0].relative_path, snapshot.entry_path);
        assert!(snapshot.declared_paths.is_empty());
        assert!(snapshot
            .errors
            .iter()
            .any(|error| error.contains("Blender") && error.contains("Originaldatei")));
        let destination = workspace.path(&format!("original-{}.blend", package.id));
        assert!(matches!(
            library.export(file.id, Some(&destination), false).unwrap(),
            ExportOutcome::Exported { .. }
        ));
        assert_eq!(fs::read(&destination).unwrap(), original);
        let bundle = workspace.path(&format!("bundle-{}", package.id));
        assert!(library.export_model(&snapshot, &[], Some(&bundle)).is_err());
        assert!(!bundle.exists());
        // A client cannot turn an entry-only snapshot into a complete bundle by clearing warnings.
        snapshot.errors.clear();
        assert!(library.export_model(&snapshot, &[], Some(&bundle)).is_err());
        assert!(!bundle.exists());
        library.trash_file(file.id).unwrap();
        assert!(library.model_resources(file.id).is_err());
        assert!(library
            .export(file.id, Some(&workspace.path("trashed.blend")), false)
            .is_err());
        assert!(!workspace.path("trashed.blend").exists());
    }
    assert_eq!(
        fs::read(source.join("Scenes/Owned.BLEND")).unwrap(),
        original
    );
}

#[test]
fn blender_external_source_changes_keep_preview_and_original_exports_guarded() {
    let workspace = Workspace::new();
    let source = workspace.path("source");
    write(&source, "Scene.blend", b"BLENDER17-01v0502 original");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.reference_folder(&source).unwrap();
    let id = model_id(&library, "Scene");
    let snapshot = library.model_resources(id).unwrap();
    let saved = source.join("original.saved");
    fs::rename(source.join("Scene.blend"), &saved).unwrap();
    write(
        &source,
        "Scene.blend",
        b"BLENDER17-01v0502 replacement changed",
    );
    let original_output = workspace.path("original.blend");
    let bundle_output = workspace.path("bundle");
    assert!(library.model_resources(id).is_err());
    assert!(library.export(id, Some(&original_output), false).is_err());
    assert!(library
        .export_model(&snapshot, &[], Some(&bundle_output))
        .is_err());
    assert!(!original_output.exists());
    assert!(!bundle_output.exists());
    assert_eq!(fs::read(saved).unwrap(), b"BLENDER17-01v0502 original");
}
