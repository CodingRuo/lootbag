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
fn view_points_become_viewport_pixels_from_the_top_left() {
    // A flipped view (WKWebView) already counts from the top.
    assert_eq!(css_point((30.0, 40.0), 600.0, true), (30.0, 40.0));
    // An unflipped view counts from the bottom.
    assert_eq!(css_point((30.0, 40.0), 600.0, false), (30.0, 560.0));
}

fn query(press: u64, window: usize, at: (f64, f64)) -> DragStep<&'static str> {
    DragStep::Query { press, window, at }
}

#[test]
fn a_held_press_asks_its_page_once_and_stages_only_a_card_answer() {
    let mut gesture = DragGesture::<&str>::default();
    // A click alone asks nothing; past the threshold the press point is queried once.
    assert_eq!(gesture.press(1, (10.0, 10.0), Some((10.0, 20.0))), None);
    assert_eq!(gesture.drag((13.0, 14.0)), DragStep::Wait);
    assert_eq!(gesture.drag((13.0, 16.0)), query(1, 1, (10.0, 20.0)));
    assert_eq!(gesture.drag((30.0, 30.0)), DragStep::Wait);
    assert!(gesture.answer(1, 1, &Some("card A")));
    // Staging is asked once; further moves wait for the payload.
    assert_eq!(gesture.drag((31.0, 30.0)), DragStep::Wait);
    assert_eq!(gesture.arm(1, "payload"), Ok(()));
    assert_eq!(gesture.drag((32.0, 30.0)), DragStep::Start("payload"));
    // Handed over once; the press is over.
    assert_eq!(gesture.drag((90.0, 90.0)), DragStep::Wait);
    assert_eq!(gesture.disarm(), None);

    // No card under the press: nothing stages, and no later answer revives it.
    gesture.press(1, (0.0, 0.0), Some((50.0, 30.0)));
    assert_eq!(gesture.drag((40.0, 0.0)), query(2, 1, (50.0, 30.0)));
    assert!(!gesture.answer(1, 2, &None::<&str>));
    assert!(!gesture.answer(1, 2, &Some("late card")));
    assert_eq!(gesture.drag((80.0, 0.0)), DragStep::Wait);
    // A press outside the webview asks nothing.
    gesture.press(1, (0.0, 0.0), None);
    assert_eq!(gesture.drag((40.0, 0.0)), DragStep::Wait);
}

#[test]
fn answers_for_another_press_window_or_a_released_press_never_stage() {
    let mut gesture = DragGesture::<&str>::default();
    // Unasked: the press has not passed the threshold yet.
    gesture.press(1, (0.0, 0.0), Some((5.0, 5.0)));
    assert!(!gesture.answer(1, 1, &Some("early")));
    assert_eq!(gesture.drag((40.0, 0.0)), query(1, 1, (5.0, 5.0)));
    // Wrong press, other window.
    assert!(!gesture.answer(1, 7, &Some("wrong press")));
    assert!(!gesture.answer(2, 1, &Some("other window")));
    // The right answer stages once; a duplicate is refused.
    assert!(gesture.answer(1, 1, &Some("card")));
    assert!(!gesture.answer(1, 1, &Some("card")));

    // Released before the answer: a late answer stages nothing.
    gesture.press(1, (0.0, 0.0), Some((5.0, 5.0)));
    assert_eq!(gesture.drag((40.0, 0.0)), query(2, 1, (5.0, 5.0)));
    assert_eq!(gesture.disarm(), None);
    assert!(!gesture.answer(1, 2, &Some("late")));
    // A payload finishing after its press was released or replaced comes back for cleanup.
    assert_eq!(gesture.arm(1, "stale"), Err("stale"));
    gesture.press(1, (0.0, 0.0), Some((5.0, 5.0)));
    gesture.drag((40.0, 0.0));
    assert!(!gesture.answer(1, 2, &Some("older press")));
    assert!(gesture.answer(1, 3, &Some("card")));
    // An armed payload that is never dragged is handed back on release or the next press.
    assert_eq!(gesture.arm(3, "armed"), Ok(()));
    assert_eq!(gesture.press(1, (0.0, 0.0), None), Some("armed"));
}

#[test]
fn discarding_a_stage_only_queues_its_batch() {
    let workspace = Workspace::new();
    let batch = workspace.path("batch");
    write(batch.join("Crate/Crate.fbx"), b"fbx");
    let (sender, removals) = std::sync::mpsc::channel();
    let trash = Trash::new(sender);
    trash.discard(&DragStage {
        batch: Some(batch.clone()),
        ..DragStage::default()
    });
    trash.discard(&DragStage::default());
    // Nothing is removed on the calling thread; the worker gets exactly the batch.
    assert!(batch.join("Crate/Crate.fbx").is_file());
    assert_eq!(
        removals.try_iter().collect::<Vec<_>>(),
        std::slice::from_ref(&batch)
    );

    let worker = Trash::spawn();
    worker.remove(batch.clone());
    for _ in 0..200 {
        if !batch.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the worker removes queued batches");
}

#[test]
fn launch_retires_published_copies_including_earlier_leftovers() {
    let workspace = Workspace::new();
    let staging = workspace.path("cache/drag-out");
    write(staging.join("1/Hit-2.wav"), b"published");
    write(workspace.path("cache/drag-out-old-crashed/x"), b"left over");
    write(workspace.path("cache/other/keep"), b"keep");
    let mut retired = retire_drag_cache(&staging);
    retired.sort();
    // The live folder is free at once for new drags; both old folders are handed to the trash.
    assert!(!staging.exists());
    assert_eq!(retired.len(), 2);
    assert!(retired.contains(&workspace.path("cache/drag-out-old-crashed")));
    assert!(retired.iter().all(|path| path.is_dir()));
    assert!(workspace.path("cache/other/keep").is_file());
    // No cache yet: nothing to retire.
    let empty = workspace.path("fresh/drag-out");
    assert!(retire_drag_cache(&empty).is_empty());
}

#[test]
fn only_one_holder_of_the_instance_lock_at_a_time() {
    let workspace = Workspace::new();
    let path = workspace.path("cache/instance.lock");
    let first = lock_instance(&path)
        .unwrap()
        .expect("first launch gets the lock");
    // A separate open file handle stands in for a concurrent second launch.
    assert!(lock_instance(&path).unwrap().is_none());
    drop(first);
    assert!(lock_instance(&path).unwrap().is_some());
}

#[test]
fn a_drag_whose_copies_exceed_the_cache_budget_is_refused_but_originals_still_drag() {
    let workspace = Workspace::new();
    write(workspace.path("Pack/Hit.wav"), &[0; 600]);
    write(workspace.path("Other/hit.wav"), &[0; 600]);
    let mut library = Library::create(&workspace.path("library")).unwrap();
    library.import_folder(&workspace.path("Pack")).unwrap();
    library.import_folder(&workspace.path("Other")).unwrap();
    let staging = workspace.path("staging");
    let pack = file(&library, "Hit.wav", "Pack");
    let other = file(&library, "hit.wav", "Other");
    // Earlier published copies already fill most of the budget.
    write(staging.join("earlier/Hit-2.wav"), &[0; 500]);

    let refused = library
        .stage_drag_within(&[plain(pack), plain(other)], &staging, 1000)
        .unwrap_err();
    assert!(refused.contains("beim nächsten Start"), "{refused}");
    assert_eq!(tree(&staging), ["earlier/Hit-2.wav"], "no partial batch");
    // A single file handed over as its original copies nothing and is unaffected.
    let original = library
        .stage_drag_within(&[plain(pack)], &staging, 1000)
        .unwrap();
    assert_eq!(original.batch, None);
    // With room left the same drag stages its renamed copy.
    assert!(library
        .stage_drag_within(&[plain(pack), plain(other)], &staging, 1100)
        .is_ok());
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
    let mut staging = Staging::new(&library, &workspace.path("staging"), DRAG_CACHE_BUDGET);
    let error = staging.model(id, &snapshot).unwrap_err();
    assert!(error.contains("geändert"), "{error}");
    assert!(staging.stage.items.is_empty());
}

#[test]
fn a_published_batch_survives_switching_to_and_dragging_from_another_library() {
    let workspace = Workspace::new();
    write(workspace.path("Pack/Hit.wav"), b"pack");
    write(workspace.path("Other/hit.wav"), b"other");
    let mut first = Library::create(&workspace.path("first")).unwrap();
    first.import_folder(&workspace.path("Pack")).unwrap();
    first.import_folder(&workspace.path("Other")).unwrap();
    let staging = workspace.path("staging");
    let published = first
        .stage_drag(
            &[
                plain(file(&first, "Hit.wav", "Pack")),
                plain(file(&first, "hit.wav", "Other")),
            ],
            &staging,
        )
        .unwrap();
    let copy = published.items[1].path.clone();

    // Switching only opens the other library; its drags stage beside the published batch.
    let mut second = Library::create(&workspace.path("second")).unwrap();
    second.import_folder(&workspace.path("Pack")).unwrap();
    second.import_folder(&workspace.path("Other")).unwrap();
    let next = second
        .stage_drag(
            &[
                plain(file(&second, "Hit.wav", "Pack")),
                plain(file(&second, "hit.wav", "Other")),
            ],
            &staging,
        )
        .unwrap();
    assert_ne!(next.batch, published.batch);
    assert_eq!(fs::read(&copy).unwrap(), b"other");
}

#[test]
fn failed_copies_and_exports_give_their_budget_back() {
    let workspace = Workspace::new();
    write(workspace.path("Pack/Hit.wav"), &[0; 600]);
    write(workspace.path("Other/hit.wav"), &[0; 600]);
    write(workspace.path("Box/Box.obj"), b"mtllib box.mtl\nv 0 0 0\n");
    write(workspace.path("Box/box.mtl"), b"newmtl a\n");
    let mut library = Library::create(&workspace.path("library")).unwrap();
    for pack in ["Pack", "Other", "Box"] {
        library.import_folder(&workspace.path(pack)).unwrap();
    }
    let mut staging = Staging::new(&library, &workspace.path("staging"), 1000);
    // Something already occupies each target, so the reserved copy and export both fail.
    write(staging.batch.join("hit-2.wav"), b"taken");
    fs::create_dir_all(staging.batch.join("Box")).unwrap();
    staging.file(file(&library, "Hit.wav", "Pack"));
    staging.file(file(&library, "hit.wav", "Other"));
    staging.file(file(&library, "Box.obj", "Box"));
    assert_eq!(
        staging.stage.problems.len(),
        2,
        "{:?}",
        staging.stage.problems
    );
    assert!(staging.stage.problems[1].contains("existiert bereits"));
    assert!(!staging.full);
    assert_eq!(staging.budget, 1000, "failed work keeps no reservation");
}
