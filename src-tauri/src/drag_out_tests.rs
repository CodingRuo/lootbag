use super::*;
use crate::library::{Asset, Canonical, CatalogQuery};
use std::sync::atomic::{AtomicU64, Ordering};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lootbag-drag-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path.canonical().unwrap())
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

fn write(path: PathBuf, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn file(library: &Library, filename: &str, package: &str) -> i64 {
    let assets: Vec<Asset> = library.query(CatalogQuery::default()).unwrap().items;
    assets
        .iter()
        .filter(|asset| asset.package_name == package)
        .flat_map(|asset| &asset.files)
        .find(|file| file.original_filename == filename)
        .unwrap()
        .id
}

fn plain(file_id: i64) -> DragSource {
    DragSource {
        file_id,
        workflow: None,
    }
}

/// Relative paths of every file below `dir`, sorted.
fn tree(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(
                    path.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    found.sort();
    found
}

#[test]
fn a_selection_delivers_originals_renamed_duplicates_and_model_bundles_and_reports_the_rest() {
    let workspace = Workspace::new();
    write(workspace.path("Pack/Hit.wav"), b"pack");
    write(
        workspace.path("Pack/OBJ/Rock.obj"),
        b"mtllib rock mat.mtl\nv 0 0 0\n",
    );
    write(
        workspace.path("Pack/OBJ/rock mat.mtl"),
        b"newmtl a\nmap_Kd stone.tga\n",
    );
    write(workspace.path("Pack/OBJ/stone.tga"), b"stone");
    write(workspace.path("Pack/OBJ/Unused.png"), b"unused");
    write(
        workspace.path("Pack/Tree/tree.gltf"),
        br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"missing.bin","byteLength":4}]}"#,
    );
    write(workspace.path("Other/hit.WAV"), b"other");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Pack")).unwrap();
    library.import_folder(&workspace.path("Other")).unwrap();
    let staging = workspace.path("staging");
    let pack_hit = file(&library, "Hit.wav", "Pack");
    let stage = library
        .stage_drag(
            &[
                plain(pack_hit),
                plain(file(&library, "hit.WAV", "Other")),
                plain(file(&library, "Rock.obj", "Pack")),
                plain(file(&library, "tree.gltf", "Pack")),
                plain(pack_hit),
            ],
            &staging,
        )
        .unwrap();

    let paths: Vec<_> = stage.items.iter().map(|item| &item.path).collect();
    let directories: Vec<_> = stage.items.iter().map(|item| item.directory).collect();
    assert_eq!(directories, [false, false, true]);
    assert_eq!(*paths[0], library.resolve_preview(pack_hit).unwrap());
    // Same name up to case: the second sound is a renamed copy, the model a bundle folder.
    assert!(paths[1].starts_with(&staging));
    assert_eq!(paths[1].file_name().unwrap(), "hit-2.WAV");
    assert_eq!(fs::read(paths[1]).unwrap(), b"other");
    assert_eq!(paths[2].file_name().unwrap(), "Rock");
    assert_eq!(
        tree(paths[2]),
        ["OBJ/Rock.obj", "OBJ/rock mat.mtl", "OBJ/stone.tga"]
    );
    assert_eq!(stage.problems.len(), 1, "{:?}", stage.problems);
    assert!(stage.problems[0].starts_with("tree.gltf: Modell-Ressourcen fehlen"));

    // Staged files stay for the recipient; a second drag gets its own folder.
    let again = library
        .stage_drag(&[plain(file(&library, "Rock.obj", "Pack"))], &staging)
        .unwrap();
    assert!(paths[2].join("OBJ/Rock.obj").is_file());
    assert_ne!(again.items[0].path.parent(), paths[2].parent());
}

#[test]
fn image_groups_drag_their_member_files() {
    let workspace = Workspace::new();
    for name in [
        "maps/Stone_Base_Color.png",
        "maps/Stone_Normal.png",
        "water/water1.png",
        "water/water2.png",
    ] {
        write(workspace.path("Textures").join(name), name.as_bytes());
    }
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Textures")).unwrap();
    let stage = library
        .stage_drag(
            &[
                DragSource {
                    file_id: file(&library, "Stone_Base_Color.png", "Textures"),
                    workflow: Some(ImageWorkflow::Material),
                },
                DragSource {
                    file_id: file(&library, "water1.png", "Textures"),
                    workflow: Some(ImageWorkflow::SpriteSequence),
                },
            ],
            &workspace.path("staging"),
        )
        .unwrap();
    let mut names: Vec<_> = stage
        .items
        .iter()
        .map(|item| {
            item.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Stone_Base_Color.png",
            "Stone_Normal.png",
            "water1.png",
            "water2.png"
        ]
    );
    assert!(stage.problems.is_empty(), "{:?}", stage.problems);
    assert!(
        !workspace.path("staging").exists(),
        "originals need no staging"
    );
}

#[test]
fn a_drag_with_nothing_deliverable_fails_with_every_reason() {
    let workspace = Workspace::new();
    write(workspace.path("Pack/Gone.wav"), b"gone");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Pack")).unwrap();
    let gone = file(&library, "Gone.wav", "Pack");
    fs::remove_file(library.resolve_preview(gone).unwrap()).unwrap();
    let error = library
        .stage_drag(&[plain(gone), plain(i64::MAX)], &workspace.path("staging"))
        .unwrap_err();
    assert!(
        error.contains("; Datei nicht im aktiven Katalog"),
        "{error}"
    );
    assert!(library.stage_drag(&[], &workspace.path("staging")).is_err());
}

#[test]
fn models_without_companions_drag_as_their_plain_file() {
    let workspace = Workspace::new();
    // Animation-only FBX and an OBJ without materials: nothing to bundle.
    write(workspace.path("Anim/Run.fbx"), b"Kaydara\0Take 001\0");
    write(workspace.path("Anim/Box.obj"), b"v 0 0 0\n");
    write(workspace.path("Other/run.FBX"), b"Kaydara\0other\0");
    write(
        workspace.path("Kit/Crate.fbx"),
        b"Kaydara\0Textures\\Crate_Wood.png\0",
    );
    write(workspace.path("Kit/Textures/Crate_Wood.png"), b"wood");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    for package in ["Anim", "Other", "Kit"] {
        library.import_folder(&workspace.path(package)).unwrap();
    }
    let run = file(&library, "Run.fbx", "Anim");
    let staging = workspace.path("staging");
    let stage = library
        .stage_drag(
            &[
                plain(run),
                plain(file(&library, "Box.obj", "Anim")),
                plain(file(&library, "run.FBX", "Other")),
                plain(file(&library, "Crate.fbx", "Kit")),
            ],
            &staging,
        )
        .unwrap();
    assert!(stage.problems.is_empty(), "{:?}", stage.problems);
    let names: Vec<_> = stage
        .items
        .iter()
        .map(|item| {
            (
                item.path.file_name().unwrap().to_str().unwrap(),
                item.directory,
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("Run.fbx", false),
            ("Box.obj", false),
            ("run-2.FBX", false),
            ("Crate", true)
        ]
    );
    assert_eq!(stage.items[0].path, library.resolve_preview(run).unwrap());
    assert!(stage.items[2].path.is_file());
    assert_eq!(
        tree(&stage.items[3].path),
        ["Crate.fbx", "Textures/Crate_Wood.png"]
    );
}

#[test]
fn a_drag_starts_only_from_the_armed_press_while_it_is_held() {
    let mut gesture = DragGesture::default();
    // Nothing pressed: neither arming nor dragging does anything.
    assert!(!gesture.arm(1, "early"));
    assert_eq!(gesture.drag((50.0, 50.0)), None);

    gesture.press((10.0, 10.0));
    let press = gesture.held().unwrap();
    // Unarmed: moving far does not start; below the threshold an armed press waits.
    assert_eq!(gesture.drag((40.0, 10.0)), None);
    assert!(gesture.arm(press, "payload"));
    assert_eq!(gesture.drag((13.0, 14.0)), None);
    assert_eq!(gesture.drag((13.0, 16.0)), Some("payload"));
    // Handed over once; the press is over.
    assert_eq!(gesture.drag((90.0, 90.0)), None);
    assert_eq!(gesture.held(), None);

    // Released before the preparation finished: the late payload is refused.
    gesture.press((0.0, 0.0));
    let released = gesture.held().unwrap();
    gesture.disarm();
    assert!(!gesture.arm(released, "late"));
    assert_eq!(gesture.drag((30.0, 0.0)), None);

    // A preparation for an earlier press never arms the next one.
    gesture.press((0.0, 0.0));
    let first = gesture.held().unwrap();
    gesture.press((0.0, 0.0));
    assert!(!gesture.arm(first, "stale"));
    assert_eq!(gesture.drag((30.0, 0.0)), None);

    // Escape or invalidation after arming disarms it.
    let second = gesture.held().unwrap();
    assert!(gesture.arm(second, "armed"));
    gesture.disarm();
    assert_eq!(gesture.drag((30.0, 0.0)), None);
}
