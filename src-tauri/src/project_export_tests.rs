use super::*;
use crate::library::{Asset, CatalogQuery, FolderMember};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "game-asset-project-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
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

const GLTF: &str = r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"tree.bin","byteLength":4}]}"#;

fn write(path: PathBuf, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// "Pack" holds Hit.wav/Hit.ogg (one asset, two variants), a glTF with buffer and Gone.wav; "Other" another Hit.wav.
fn fixture(workspace: &Workspace) -> Library {
    write(workspace.path("Pack/Hit.wav"), b"pack wav");
    write(workspace.path("Pack/Hit.ogg"), b"pack ogg");
    write(workspace.path("Pack/Gone.wav"), b"gone");
    write(workspace.path("Pack/Tree/tree.gltf"), GLTF.as_bytes());
    write(workspace.path("Pack/Tree/tree.bin"), b"mesh");
    write(workspace.path("Other/Hit.wav"), b"other wav");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Pack")).unwrap();
    library.import_folder(&workspace.path("Other")).unwrap();
    library
}

fn assets(library: &Library) -> Vec<Asset> {
    library.query(CatalogQuery::default()).unwrap().items
}

fn find(library: &Library, name: &str, package: &str) -> Asset {
    assets(library)
        .into_iter()
        .find(|asset| asset.name == name && asset.package_name == package)
        .unwrap()
}

#[test]
fn migration_keeps_old_memberships_and_adds_variant_and_role() {
    let workspace = Workspace::new();
    let mut library = fixture(&workspace);
    let folder = library.create_folder("Game", None).unwrap();
    let hit = find(&library, "Hit", "Pack");
    library.add_to_folder(folder.id, hit.id).unwrap();
    drop(library);
    let connection = Connection::open(workspace.path("library/catalog.sqlite3")).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
            CREATE TABLE old_members (folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE, PRIMARY KEY(folder_id, asset_id));
            INSERT INTO old_members SELECT folder_id, asset_id FROM folder_assets;
            DROP TABLE folder_assets; ALTER TABLE old_members RENAME TO folder_assets;
            PRAGMA user_version = 2;",
        )
        .unwrap();
    drop(connection);
    assert!(Library::open_read_only(&workspace.path("library")).is_err());
    let mut library = Library::open(&workspace.path("library")).unwrap();
    assert_eq!(
        find(&library, "Hit", "Pack").folder_members,
        [FolderMember {
            folder_id: folder.id,
            file_id: None,
            role: None
        }]
    );
    library
        .set_folder_member(folder.id, hit.id, None, Some("Treffer"))
        .unwrap();
    drop(library);
    let library = Library::open(&workspace.path("library")).unwrap();
    assert_eq!(
        find(&library, "Hit", "Pack").folder_members[0]
            .role
            .as_deref(),
        Some("Treffer")
    );
}

#[test]
fn role_and_variant_are_saved_validated_and_fall_back_when_the_variant_is_trashed() {
    let workspace = Workspace::new();
    let mut library = fixture(&workspace);
    let folder = library.create_folder("Game", None).unwrap();
    let hit = find(&library, "Hit", "Pack");
    let other = find(&library, "Hit", "Other");
    let ogg = hit
        .files
        .iter()
        .find(|file| file.format == "OGG")
        .unwrap()
        .id;
    library
        .set_folder_member(folder.id, hit.id, Some(ogg), Some("  Wald-Ambience "))
        .unwrap();
    assert_eq!(
        find(&library, "Hit", "Pack").folder_members,
        [FolderMember {
            folder_id: folder.id,
            file_id: Some(ogg),
            role: Some("Wald-Ambience".into())
        }]
    );
    assert!(library
        .set_folder_member(folder.id, hit.id, None, Some(&"x".repeat(81)))
        .is_err());
    assert!(library
        .set_folder_member(folder.id, hit.id, Some(other.files[0].id), None)
        .is_err());
    library.trash_file(ogg).unwrap();
    let member = &find(&library, "Hit", "Pack").folder_members[0];
    assert_eq!(
        (member.file_id, member.role.as_deref()),
        (None, Some("Wald-Ambience"))
    );
    library
        .set_folder_member(folder.id, hit.id, None, Some(""))
        .unwrap();
    assert_eq!(find(&library, "Hit", "Pack").folder_members[0].role, None);
}

#[test]
fn export_copies_chosen_variants_and_model_companions_with_a_manifest() {
    let workspace = Workspace::new();
    let mut library = fixture(&workspace);
    let game = library.create_folder("Game", None).unwrap();
    let sfx = library.create_folder("SFX", Some(game.id)).unwrap();
    let hit = find(&library, "Hit", "Pack");
    // OGG sorts first and is the default variant; WAV is chosen explicitly.
    let wav = hit.files.iter().find(|file| file.format == "WAV").unwrap();
    let other = find(&library, "Hit", "Other");
    let tree = find(&library, "tree", "Pack");
    let gone = find(&library, "Gone", "Pack");
    library
        .set_folder_member(sfx.id, hit.id, Some(wav.id), Some("Held"))
        .unwrap();
    library.add_to_folder(sfx.id, other.id).unwrap();
    library
        .set_folder_member(game.id, tree.id, None, Some("Baum"))
        .unwrap();
    library.add_to_folder(game.id, gone.id).unwrap();
    fs::remove_file(workspace.path("library").join(&gone.files[0].relative_path)).unwrap();
    let originals = assets(&library)
        .iter()
        .flat_map(|asset| asset.files.clone())
        .filter(|file| file.available)
        .map(|file| {
            let path = workspace.path("library").join(&file.relative_path);
            (fs::read(&path).unwrap(), path)
        })
        .collect::<Vec<_>>();

    let plan = library.plan_project_export(game.id).unwrap();
    assert_eq!((plan.project.as_str(), plan.asset_count), ("Game", 4));
    assert_eq!(plan.missing.len(), 1);
    assert!(
        plan.missing[0].starts_with("Gone (Pack)"),
        "{:?}",
        plan.missing
    );

    let exports = workspace.path("exports");
    fs::create_dir(&exports).unwrap();
    let ExportOutcome::Exported { path, .. } = library.export_project(game.id, &exports).unwrap()
    else {
        panic!("not exported");
    };
    let out = exports.join("Game");
    assert_eq!(PathBuf::from(path), out);
    // Same file name in one export folder gets a deterministic suffix (catalog order: Pack first).
    assert_eq!(fs::read(out.join("SFX/Hit.wav")).unwrap(), b"pack wav");
    assert_eq!(fs::read(out.join("SFX/Hit-2.wav")).unwrap(), b"other wav");
    assert!(!out.join("SFX/Hit.ogg").exists());
    assert_eq!(
        fs::read(out.join("tree/Tree/tree.gltf")).unwrap(),
        GLTF.as_bytes()
    );
    assert_eq!(fs::read(out.join("tree/Tree/tree.bin")).unwrap(), b"mesh");
    let manifest: Value =
        serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["project"], "Game");
    assert!(manifest["exportedAt"].as_str().unwrap().ends_with('Z'));
    let entries = manifest["assets"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    let first = &entries[0];
    assert_eq!(first["assetId"], tree.id);
    assert_eq!(
        (first["role"].as_str(), first["kind"].as_str()),
        (Some("Baum"), Some("model"))
    );
    assert_eq!(first["originalPath"], "Tree/tree.gltf");
    assert_eq!(
        first["exportedFiles"],
        json!(["tree/Tree/tree.bin", "tree/Tree/tree.gltf"])
    );
    assert_eq!(first["license"], Value::Null);
    let held = entries
        .iter()
        .find(|entry| entry["assetId"] == hit.id)
        .unwrap();
    assert_eq!(
        held["variant"],
        json!({ "fileId": wav.id, "format": "WAV", "file": "Hit.wav" })
    );
    assert_eq!(held["exportedFiles"], json!(["SFX/Hit.wav"]));
    let renamed = entries
        .iter()
        .find(|entry| entry["assetId"] == other.id)
        .unwrap();
    assert_eq!(renamed["exportedFiles"], json!(["SFX/Hit-2.wav"]));
    assert_eq!(held["package"], "Pack");
    assert_eq!(manifest["missing"][0]["assetId"], gone.id);

    let error = library.export_project(game.id, &exports).unwrap_err();
    assert!(error.contains("existiert bereits"), "{error}");
    assert!(library
        .export_project(game.id, &workspace.path("library/packages"))
        .is_err());
    assert!(!workspace.path("library/packages/Game").exists());
    for (bytes, path) in originals {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn iso_timestamps_are_utc() {
    assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso_utc(951_782_400 + 3_723), "2000-02-29T01:02:03Z");
}

#[test]
fn fbx_and_obj_exports_include_referenced_textures_and_materials_only() {
    let workspace = Workspace::new();
    write(
        workspace.path("Kit/FBX/Crate.fbx"),
        // Also names the copied PNG and an unrelated "Old_Crate_Wood": the PSD still needs its note.
        b"Kaydara\0..\\..\\Textures\\Crate_Wood.PSD\0Crate_Wood.png\0Old_Crate_Wood.tga\0",
    );
    write(workspace.path("Kit/Textures/Crate_Wood.png"), b"wood");
    write(workspace.path("Kit/Textures/Unused.png"), b"unused");
    write(
        workspace.path("Kit/OBJ/Rock.obj"),
        b"mtllib rock mat.mtl\nv 0 0 0\n",
    );
    write(
        workspace.path("Kit/OBJ/rock mat.mtl"),
        b"newmtl a\nmap_Kd stone.tga\n",
    );
    write(workspace.path("Kit/OBJ/stone.tga"), b"stone");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Kit")).unwrap();
    let folder = library.create_folder("Props", None).unwrap();
    for name in ["Crate", "Rock"] {
        library
            .add_to_folder(folder.id, find(&library, name, "Kit").id)
            .unwrap();
    }
    let plan = library.plan_project_export(folder.id).unwrap();
    assert!(plan.missing.is_empty());
    assert_eq!(plan.substitutions, 1);
    let exports = workspace.path("exports");
    fs::create_dir(&exports).unwrap();
    library.export_project(folder.id, &exports).unwrap();
    let manifest: Value =
        serde_json::from_slice(&fs::read(exports.join("Props/manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["assets"][0]["exportedFiles"],
        json!(["Crate/FBX/Crate.fbx", "Crate/Textures/Crate_Wood.png"])
    );
    assert_eq!(
        manifest["assets"][1]["exportedFiles"],
        json!([
            "Rock/OBJ/Rock.obj",
            "Rock/OBJ/rock mat.mtl",
            "Rock/OBJ/stone.tga"
        ])
    );
    assert_eq!(
        manifest["assets"][0]["notes"],
        json!([
            "Textur Crate_Wood.PSD ersetzt durch Crate_Wood.png – Referenz im Modell unverändert"
        ])
    );
    assert_eq!(manifest["assets"][1]["notes"], json!([]));
    assert!(!exports.join("Props/Crate/Textures/Unused.png").exists());
    assert_eq!(
        fs::read(exports.join("Props/Rock/OBJ/stone.tga")).unwrap(),
        b"stone"
    );
}

#[test]
fn a_subfolder_named_like_the_manifest_moves_the_manifest_aside() {
    let workspace = Workspace::new();
    let mut library = fixture(&workspace);
    let game = library.create_folder("Game", None).unwrap();
    let sub = library
        .create_folder("Manifest.JSON", Some(game.id))
        .unwrap();
    library
        .add_to_folder(sub.id, find(&library, "Gone", "Pack").id)
        .unwrap();
    let exports = workspace.path("exports");
    fs::create_dir(&exports).unwrap();
    library.export_project(game.id, &exports).unwrap();
    let out = exports.join("Game");
    assert_eq!(
        fs::read(out.join("Manifest.JSON/Gone.wav")).unwrap(),
        b"gone"
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(out.join("manifest-2.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["assets"][0]["exportedFiles"],
        json!(["Manifest.JSON/Gone.wav"])
    );
}

#[cfg(unix)]
#[test]
fn failed_copy_keeps_the_partial_export_and_names_it() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = Workspace::new();
    let mut library = fixture(&workspace);
    let folder = library.create_folder("Game", None).unwrap();
    let gone = find(&library, "Gone", "Pack");
    let other = find(&library, "Hit", "Other");
    library.add_to_folder(folder.id, gone.id).unwrap();
    library.add_to_folder(folder.id, other.id).unwrap();
    // "Gone" sorts first and copies; "Hit" cannot be read.
    let unreadable = workspace
        .path("library")
        .join(&other.files[0].relative_path);
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    let exports = workspace.path("exports");
    fs::create_dir(&exports).unwrap();
    let error = library.export_project(folder.id, &exports).unwrap_err();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
    let out = exports.join("Game");
    assert!(
        error.starts_with(&format!("Export unvollständig: {}", out.display())),
        "{error}"
    );
    // Nothing in the export folder is deleted: a file already there (ours or the user's) stays.
    assert_eq!(fs::read(out.join("Gone.wav")).unwrap(), b"gone");
}

#[test]
fn texture_notes_ignore_the_model_itself_and_other_names_containing_the_stem() {
    let workspace = Workspace::new();
    // Own file name, an LOD name and "Old Wood.tga" must not be mistaken for a missing "Wood" texture.
    write(
        workspace.path("Kit/Wood.fbx"),
        b"Kaydara\0Wood.fbx\0Wood.LOD0.fbx\0Textures\\Wood.png\0Textures\\Old Wood.tga\0",
    );
    write(workspace.path("Kit/Textures/Wood.png"), b"wood");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Kit")).unwrap();
    let folder = library.create_folder("Props", None).unwrap();
    library
        .add_to_folder(folder.id, find(&library, "Wood", "Kit").id)
        .unwrap();
    let plan = library.plan_project_export(folder.id).unwrap();
    assert_eq!(plan.substitutions, 0);
}

#[test]
fn mtl_texture_names_with_spaces_keep_their_whole_name() {
    let workspace = Workspace::new();
    write(
        workspace.path("Kit/Rock.obj"),
        b"mtllib rock.mtl\nv 0 0 0\n",
    );
    write(
        workspace.path("Kit/rock.mtl"),
        b"newmtl a\nmap_Kd Old Wood.tga\nmap_Bump textures/Bark Moss.tga\n",
    );
    // "Old Wood.tga" is missing but "Old Wood.png" exists: a note. "Wood.png" alone must not count.
    write(workspace.path("Kit/Old Wood.png"), b"old");
    write(workspace.path("Kit/Wood.png"), b"wood");
    write(workspace.path("Kit/textures/Bark Moss.tga"), b"bark");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Kit")).unwrap();
    let folder = library.create_folder("Props", None).unwrap();
    library
        .add_to_folder(folder.id, find(&library, "Rock", "Kit").id)
        .unwrap();
    let plan = library.plan_project_export(folder.id).unwrap();
    assert_eq!(plan.substitutions, 1);
    let exports = workspace.path("exports");
    fs::create_dir(&exports).unwrap();
    library.export_project(folder.id, &exports).unwrap();
    let manifest: Value =
        serde_json::from_slice(&fs::read(exports.join("Props/manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["assets"][0]["notes"],
        json!(["Textur Old Wood.tga ersetzt durch Old Wood.png – Referenz im Modell unverändert"])
    );
}

#[test]
fn package_info_is_saved_listed_and_removed_with_the_package() {
    let workspace = Workspace::new();
    let mut library = fixture(&workspace);
    write(workspace.path("Lic/LICENSE.txt"), b"cc0");
    write(workspace.path("Lic/Docs/readme.md"), b"read");
    write(workspace.path("Lic/Docs/Deep/License.txt"), b"too deep");
    write(workspace.path("Lic/Beep.wav"), b"beep");
    let lic = library.import_folder(&workspace.path("Lic")).unwrap();
    let pack = find(&library, "Hit", "Pack").package_id;
    let info = PackageInfo {
        store: Some("itch.io".into()),
        license: Some("CC0".into()),
        purchased: Some("2025-01-31".into()),
        ..Default::default()
    };
    library.set_package_info(pack, &info).unwrap();
    assert_eq!(library.get_package_info(pack).unwrap().info, info);
    let listed = library.packages().unwrap();
    assert_eq!(listed.iter().find(|p| p.id == pack).unwrap().info, info);
    assert!(library
        .set_package_info(
            pack,
            &PackageInfo {
                url: Some("www.x.de".into()),
                ..Default::default()
            }
        )
        .is_err());
    assert!(library.set_package_info(9_999, &info).is_err());
    assert_eq!(
        library.get_package_info(lic.id).unwrap().license_files,
        ["LICENSE.txt", "Docs/readme.md"]
    );
    // Clearing every field removes the row.
    library.set_package_info(lic.id, &info).unwrap();
    library
        .set_package_info(lic.id, &PackageInfo::default())
        .unwrap();
    let rows = |library: &Library| -> i64 {
        library
            .connection
            .query_row("SELECT count(*) FROM package_info", [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(rows(&library), 1);
    library.trash_package(pack).unwrap();
    assert!(library.set_package_info(pack, &info).is_err());
    assert_eq!(rows(&library), 1);
    library.empty_trash().unwrap();
    assert_eq!(rows(&library), 0);
}

#[test]
fn export_writes_credits_and_licence_fields_and_counts_packages_without_licence() {
    let workspace = Workspace::new();
    let mut library = fixture(&workspace);
    let game = library.create_folder("Game", None).unwrap();
    // A subfolder named like the credits file moves CREDITS.md aside.
    let sub = library.create_folder("credits.md", Some(game.id)).unwrap();
    let hit = find(&library, "Hit", "Pack");
    let tree = find(&library, "tree", "Pack");
    let other = find(&library, "Hit", "Other");
    library.add_to_folder(game.id, hit.id).unwrap();
    library.add_to_folder(game.id, tree.id).unwrap();
    library.add_to_folder(sub.id, other.id).unwrap();
    library
        .set_package_info(
            hit.package_id,
            &PackageInfo {
                store: Some("Kenney".into()),
                url: Some("https://kenney.nl".into()),
                license: Some("CC0".into()),
                author: Some("Kenney Vleugels".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        library
            .plan_project_export(game.id)
            .unwrap()
            .packages_without_license,
        1
    );
    let exports = workspace.path("exports");
    fs::create_dir(&exports).unwrap();
    library.export_project(game.id, &exports).unwrap();
    let out = exports.join("Game");
    let credits = fs::read_to_string(out.join("CREDITS-2.md")).unwrap();
    assert_eq!(
        credits,
        "# Credits – Game\n\n## Other\n\n- Lizenz: nicht hinterlegt – bitte prüfen\n- Assets:\n  - Hit\n\n\
         ## Pack\n\n- Urheber: Kenney Vleugels\n- Lizenz: CC0\n- Shop: Kenney – https://kenney.nl\n- Assets:\n  - Hit\n  - tree\n"
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
    let entry = |id: i64| {
        manifest["assets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["assetId"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(
        (
            entry(hit.id)["license"].as_str(),
            entry(hit.id)["author"].as_str(),
            entry(hit.id)["store"].as_str(),
            entry(hit.id)["url"].as_str()
        ),
        (
            Some("CC0"),
            Some("Kenney Vleugels"),
            Some("Kenney"),
            Some("https://kenney.nl")
        )
    );
    assert_eq!(entry(other.id)["license"], Value::Null);
}
