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
fn a_drag_stages_once_past_the_threshold_and_starts_only_from_the_held_press() {
    let mut gesture = DragGesture::<&str, &str>::default();
    // Nothing pressed: neither requests, arming nor dragging do anything.
    assert!(!gesture.request(1, 1, "early"));
    assert_eq!(gesture.arm(1, "early"), Err("early"));
    assert_eq!(gesture.drag((50.0, 50.0)), DragStep::Wait);

    let (press, dropped) = gesture.press(1, (10.0, 10.0));
    assert_eq!(dropped, None);
    // Not requested: moving far stages nothing; a click alone never stages.
    assert_eq!(gesture.drag((40.0, 10.0)), DragStep::Wait);
    assert!(gesture.request(1, press, "cards"));
    assert!(!gesture.request(1, press, "again"), "one request per press");
    assert_eq!(gesture.drag((13.0, 14.0)), DragStep::Wait);
    assert_eq!(gesture.drag((13.0, 16.0)), DragStep::Stage(press, "cards"));
    // Staging is asked once; further moves wait for the payload.
    assert_eq!(gesture.drag((30.0, 30.0)), DragStep::Wait);
    assert_eq!(gesture.arm(press, "payload"), Ok(()));
    assert_eq!(gesture.drag((31.0, 30.0)), DragStep::Start("payload"));
    // Handed over once; the press is over.
    assert_eq!(gesture.drag((90.0, 90.0)), DragStep::Wait);
    assert_eq!(gesture.disarm(), None);
}

#[test]
fn a_delayed_request_or_payload_never_binds_to_a_later_press_or_another_window() {
    let mut gesture = DragGesture::<&str, &str>::default();
    // Card A is clicked and released; its request is still in flight when B is pressed.
    let (a, _) = gesture.press(1, (0.0, 0.0));
    assert_eq!(gesture.disarm(), None);
    let (b, _) = gesture.press(1, (100.0, 0.0));
    assert!(
        !gesture.request(1, a, "card A"),
        "A's late request names press A"
    );
    assert_eq!(gesture.drag((130.0, 0.0)), DragStep::Wait);
    // B's own request from another window is refused too; from its window it binds.
    assert!(!gesture.request(2, b, "card B"));
    assert!(gesture.request(1, b, "card B"));
    assert_eq!(gesture.drag((130.0, 0.0)), DragStep::Stage(b, "card B"));

    // A payload finishing after its press was released or replaced comes back for cleanup.
    assert_eq!(gesture.arm(a, "stale"), Err("stale"));
    let (c, _) = gesture.press(1, (0.0, 0.0));
    assert_eq!(gesture.arm(b, "late"), Err("late"));
    assert!(gesture.request(1, c, "card C"));
    assert_eq!(gesture.drag((30.0, 0.0)), DragStep::Stage(c, "card C"));
    // An armed payload that is never dragged is handed back on release or the next press.
    assert_eq!(gesture.arm(c, "armed"), Ok(()));
    assert_eq!(gesture.disarm(), Some("armed"));
    let (d, _) = gesture.press(1, (0.0, 0.0));
    assert!(gesture.request(1, d, "card D"));
    assert_eq!(gesture.drag((30.0, 0.0)), DragStep::Stage(d, "card D"));
    assert_eq!(gesture.arm(d, "armed"), Ok(()));
    assert_eq!(gesture.press(1, (0.0, 0.0)).1, Some("armed"));
}

#[test]
fn published_batches_are_removed_only_beyond_the_newest_and_after_their_grace() {
    let mut retention = Retention::new();
    let start = Instant::now();
    let batch = |index: usize| PathBuf::from(format!("batch-{index}"));
    for index in 0..KEEP_BATCHES + 2 {
        retention.published(batch(index));
        assert_eq!(
            retention.ended(Some(&batch(index)), start),
            Vec::<PathBuf>::new()
        );
    }
    // Within the grace period nothing goes, even beyond the newest few.
    let soon = start + BATCH_GRACE / 2;
    assert!(retention.ended(None, soon).is_empty());
    // Afterwards only the oldest go; the newest stay for their recipients.
    let later = start + BATCH_GRACE;
    assert_eq!(retention.ended(None, later), [batch(0), batch(1)]);
    assert!(retention.ended(None, later).is_empty());
    // A batch whose drag is still running is never removed.
    retention.published(batch(9));
    for index in 10..10 + KEEP_BATCHES {
        retention.published(batch(index));
        retention.ended(Some(&batch(index)), later);
    }
    let removed = retention.ended(None, later + BATCH_GRACE);
    assert!(!removed.contains(&batch(9)), "{removed:?}");
}

#[test]
fn a_failed_preparation_leaves_no_copies_and_a_successful_one_names_its_batch() {
    let workspace = Workspace::new();
    write(workspace.path("Pack/Hit.wav"), b"pack");
    write(workspace.path("Other/hit.wav"), b"other");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Pack")).unwrap();
    library.import_folder(&workspace.path("Other")).unwrap();
    let staging = workspace.path("staging");
    let pack = file(&library, "Hit.wav", "Pack");
    let other = file(&library, "hit.wav", "Other");

    let stage = library
        .stage_drag(&[plain(pack), plain(other)], &staging)
        .unwrap();
    let batch = stage.batch.clone().unwrap();
    assert!(stage.items[1].path.starts_with(&batch));
    let only_originals = library.stage_drag(&[plain(pack)], &staging).unwrap();
    assert_eq!(only_originals.batch, None);

    // A refused model bundle had already created its batch folder: nothing is left behind.
    write(
        workspace.path("Tree/tree.gltf"),
        br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"missing.bin","byteLength":4}]}"#,
    );
    library.import_folder(&workspace.path("Tree")).unwrap();
    fs::remove_dir_all(&staging).unwrap();
    fs::create_dir_all(&staging).unwrap();
    let tree = file(&library, "tree.gltf", "Tree");
    assert!(library.stage_drag(&[plain(tree)], &staging).is_err());
    assert_eq!(
        fs::read_dir(&staging).unwrap().count(),
        0,
        "no batch left behind"
    );
}

#[test]
fn a_plain_model_that_changed_since_its_snapshot_is_refused() {
    let workspace = Workspace::new();
    write(workspace.path("Pack/Box.obj"), b"v 0 0 0\n");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Pack")).unwrap();
    let id = file(&library, "Box.obj", "Pack");
    let snapshot = library.model_resources(id).unwrap();
    // Between snapshot and hand-over the model gains a material it now depends on.
    let path = library.resolve_preview(id).unwrap();
    fs::write(&path, b"mtllib box.mtl\nv 0 0 0\n").unwrap();
    write(path.with_file_name("box.mtl"), b"newmtl a\n");
    let mut staging = Staging::new(&library, &workspace.path("staging"));
    let error = staging.model(id, &snapshot).unwrap_err();
    assert!(error.contains("geändert"), "{error}");
    assert!(staging.stage.items.is_empty());
}
