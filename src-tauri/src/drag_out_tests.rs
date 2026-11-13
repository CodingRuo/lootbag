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

    assert_eq!(stage.paths.len(), 3);
    assert_eq!(stage.paths[0], library.resolve_preview(pack_hit).unwrap());
    // Same name up to case: the second sound is a renamed copy, the model a bundle folder.
    assert!(stage.paths[1].starts_with(&staging));
    assert_eq!(stage.paths[1].file_name().unwrap(), "hit-2.WAV");
    assert_eq!(fs::read(&stage.paths[1]).unwrap(), b"other");
    assert_eq!(stage.paths[2].file_name().unwrap(), "Rock");
    assert_eq!(
        tree(&stage.paths[2]),
        ["OBJ/Rock.obj", "OBJ/rock mat.mtl", "OBJ/stone.tga"]
    );
    assert_eq!(stage.problems.len(), 1, "{:?}", stage.problems);
    assert!(stage.problems[0].starts_with("tree.gltf: Modell-Ressourcen fehlen"));

    // Staged files stay for the recipient; a second drag gets its own folder.
    let again = library
        .stage_drag(&[plain(file(&library, "Rock.obj", "Pack"))], &staging)
        .unwrap();
    assert!(stage.paths[2].join("OBJ/Rock.obj").is_file());
    assert_ne!(again.paths[0].parent(), stage.paths[2].parent());
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
        .paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
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
