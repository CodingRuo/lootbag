use super::*;
use crate::library::{CatalogQuery, Library};
use std::{
    fs,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 8, 215, 99, 248, 207, 192, 240, 31, 0, 5,
    0, 1, 255, 114, 156, 82, 103, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
struct Fixture {
    root: PathBuf,
    source: PathBuf,
    library: Library,
    snapshot: ModelResources,
}
impl Fixture {
    fn new(reference: bool) -> Self {
        Self::import(if reference { "reference" } else { "managed" })
    }
    fn import(mode: &str) -> Self {
        Self::create(mode, PNG)
    }
    fn create(mode: &str, png: &[u8]) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "sprite-guards-{}-{}-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("Owned Sheet");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested/sheet.png"), png).unwrap();
        let mut library = Library::create(&root.join("Library")).unwrap();
        match mode {
            "reference" => {
                library.reference_folder(&source).unwrap();
            }
            "zip" => {
                let archive = root.join("sheet.zip");
                let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
                zip.start_file("nested/sheet.png", zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(png).unwrap();
                zip.finish().unwrap();
                library.import_zip(&archive).unwrap();
            }
            _ => {
                library.import_folder(&source).unwrap();
            }
        }
        let file = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
        let snapshot = library.sheet_resources(file).unwrap();
        Self {
            root,
            source,
            library,
            snapshot,
        }
    }
    fn bytes(&self, destination: &Path, mode: &str) -> Vec<u8> {
        let json=serde_json::to_vec(&serde_json::json!({"snapshotId":self.snapshot.snapshot_id,"fileId":self.snapshot.file_id,"destination":destination,"width":1,"height":1,"layout":{"columns":1,"rows":1,"left":0,"top":0,"right":0,"bottom":0,"gapX":0,"gapY":0},"fps":10,"mode":mode,"lengths":[PNG.len()]})).unwrap();
        let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
        bytes.extend(json);
        bytes.extend(PNG);
        bytes
    }
    fn assert_clean(&self) {
        assert_eq!(fs::read(self.source.join("nested/sheet.png")).unwrap(), PNG);
        assert!(!fs::read_dir(&self.root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".game-assets-sprite-")));
    }
}

#[test]
fn six_authored_alpha_frames_and_tight_atlas_publish_exact_pngs_and_literal_coordinates() {
    // Owned primary-color alpha fixture; source/output PNGs independently decoded with FFmpeg.
    let original = include_bytes!("../tests/fixtures/sprite-padded-source.png");
    let fixture = Fixture::create("managed", original);
    let frames: &[&[u8]] = &[
        include_bytes!("../tests/fixtures/sprite-frame-0001.png"),
        include_bytes!("../tests/fixtures/sprite-frame-0002.png"),
        include_bytes!("../tests/fixtures/sprite-frame-0003.png"),
        include_bytes!("../tests/fixtures/sprite-frame-0004.png"),
        include_bytes!("../tests/fixtures/sprite-frame-0005.png"),
        include_bytes!("../tests/fixtures/sprite-frame-0006.png"),
    ];
    let atlas: &[&[u8]] = &[include_bytes!("../tests/fixtures/sprite-atlas.png")];
    for (mode, images) in [("frames", frames), ("atlas", atlas)] {
        let destination = fixture.root.join(mode);
        let header = serde_json::json!({"snapshotId":fixture.snapshot.snapshot_id,"fileId":fixture.snapshot.file_id,"destination":destination,"width":25,"height":17,"layout":{"columns":3,"rows":2,"left":2,"top":3,"right":3,"bottom":2,"gapX":1,"gapY":2},"fps":10,"mode":mode,"lengths":images.iter().map(|p|p.len()).collect::<Vec<_>>()});
        let json = serde_json::to_vec(&header).unwrap();
        let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
        bytes.extend(json);
        for image in images {
            bytes.extend(*image);
        }
        let payload = SpritePayload::parse(&bytes).unwrap();
        fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || Ok(()))
            .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(destination.join("sprite.json")).unwrap()).unwrap();
        assert_eq!(manifest["fps"], 10);
        assert_eq!(
            manifest["canvas"],
            serde_json::json!({"width":25,"height":17})
        );
        assert_eq!(
            manifest["frames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|frame| (
                    frame["source"]["x"].as_u64().unwrap(),
                    frame["source"]["y"].as_u64().unwrap(),
                    frame["source"]["width"].as_u64().unwrap(),
                    frame["source"]["height"].as_u64().unwrap()
                ))
                .collect::<Vec<_>>(),
            vec![
                (2, 3, 6, 5),
                (9, 3, 6, 5),
                (16, 3, 6, 5),
                (2, 10, 6, 5),
                (9, 10, 6, 5),
                (16, 10, 6, 5)
            ]
        );
        if mode == "atlas" {
            assert_eq!(
                manifest["outputCanvas"],
                serde_json::json!({"width":18,"height":10})
            );
            assert_eq!(
                manifest["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|frame| (
                        frame["output"]["x"].as_u64().unwrap(),
                        frame["output"]["y"].as_u64().unwrap()
                    ))
                    .collect::<Vec<_>>(),
                vec![(0, 0), (6, 0), (12, 0), (0, 5), (6, 5), (12, 5)]
            );
        }
        for (index, image) in images.iter().enumerate() {
            let name = if mode == "atlas" {
                "atlas.png".into()
            } else {
                format!("frame-{:04}.png", index + 1)
            };
            assert_eq!(fs::read(destination.join(name)).unwrap(), *image);
        }
    }
    assert_eq!(
        fs::read(
            fixture
                .library
                .resolve_preview(fixture.snapshot.file_id)
                .unwrap()
        )
        .unwrap(),
        original
    );
}
fn identity(path: &Path) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn every_precommit_failure_leaves_no_final_folder_and_recovers_without_changing_sources() {
    let fixture = Fixture::new(false);
    for phase in ["write"] {
        let destination = fixture.root.join(phase);
        let bytes = fixture.bytes(&destination, "frames");
        let payload = SpritePayload::parse(&bytes).unwrap();
        TEST_FAILURE.with(|point| point.set(Some(phase)));
        assert!(fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || Ok(()))
            .is_err());
        TEST_FAILURE.with(|point| point.set(None));
        assert!(!destination.exists());
        fixture.assert_clean();
        fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || Ok(()))
            .unwrap();
        assert_eq!(fs::read(destination.join("frame-0001.png")).unwrap(), PNG);
    }
}

#[test]
fn zip_sheet_exports_atlas_with_immutable_original_folder_provenance() {
    let fixture = Fixture::import("zip");
    let destination = fixture.root.join("zip-atlas");
    let bytes = fixture.bytes(&destination, "atlas");
    let payload = SpritePayload::parse(&bytes).unwrap();
    fixture
        .library
        .export_sprite(&fixture.snapshot, &payload, || Ok(()))
        .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join("sprite.json")).unwrap()).unwrap();
    assert_eq!(manifest["mode"], "atlas");
    assert_eq!(manifest["source"]["originalFolder"], "nested");
    assert_eq!(manifest["frames"][0]["filename"], "atlas.png");
    assert_eq!(fs::read(destination.join("atlas.png")).unwrap(), PNG);
    assert_eq!(
        fs::read(
            fixture
                .library
                .resolve_preview(fixture.snapshot.file_id)
                .unwrap()
        )
        .unwrap(),
        PNG
    );
    fixture.assert_clean();
}

#[test]
fn relinking_external_source_and_using_another_library_invalidates_sheet_delivery() {
    let fixture = Fixture::new(true);
    let destination = fixture.root.join("relinked");
    let bytes = fixture.bytes(&destination, "frames");
    let payload = SpritePayload::parse(&bytes).unwrap();
    let moved = fixture.root.join("Moved Source");
    fs::rename(&fixture.source, &moved).unwrap();
    let package = fixture.library.packages().unwrap()[0].id;
    let mut other = Library::open(fixture.library.root()).unwrap();
    other.relink_source(package, &moved).unwrap();
    assert!(fixture
        .library
        .export_sprite(&fixture.snapshot, &payload, || Ok(()))
        .is_err());
    assert!(!destination.exists());
    assert_eq!(fs::read(moved.join("nested/sheet.png")).unwrap(), PNG);
    let another = Library::create(&fixture.root.join("Another Library")).unwrap();
    assert!(another
        .export_sprite(&fixture.snapshot, &payload, || Ok(()))
        .is_err());
    assert!(!destination.exists());
}

#[test]
fn reference_and_managed_sources_reject_cancel_trash_and_changes_before_publication() {
    for reference in [false, true] {
        let fixture = Fixture::new(reference);
        let destination = fixture.root.join("cancelled");
        let bytes = fixture.bytes(&destination, "atlas");
        let payload = SpritePayload::parse(&bytes).unwrap();
        assert!(fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || Err("cancelled".into()))
            .is_err());
        assert!(!destination.exists());
        fixture.assert_clean();
        let destination = fixture.root.join("changed");
        let bytes = fixture.bytes(&destination, "frames");
        let payload = SpritePayload::parse(&bytes).unwrap();
        let source = fixture
            .library
            .resolve_preview(fixture.snapshot.file_id)
            .unwrap();
        assert!(fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || {
                fs::write(&source, b"changed").unwrap();
                Ok(())
            })
            .is_err());
        assert!(!destination.exists());
        assert!(!fs::read_dir(&fixture.root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".game-assets-sprite-")));
        fs::write(&source, PNG).unwrap();
        // External imports intentionally reject a rewritten original, even if its bytes match again.
        if reference {
            assert!(fixture
                .library
                .sheet_resources(fixture.snapshot.file_id)
                .is_err());
        }
        let trash = Fixture::new(reference);
        let destination = trash.root.join("trashed");
        let bytes = trash.bytes(&destination, "frames");
        let payload = SpritePayload::parse(&bytes).unwrap();
        let mut other = Library::open(trash.library.root()).unwrap();
        other.trash_file(trash.snapshot.file_id).unwrap();
        assert!(trash
            .library
            .export_sprite(&trash.snapshot, &payload, || Ok(()))
            .is_err());
        assert!(!destination.exists());
        trash.assert_clean();
    }
}

#[test]
fn unsafe_parents_existing_files_directories_and_symlinks_remain_unchanged() {
    let fixture = Fixture::new(true);
    for destination in [
        fixture.source.join("unsafe"),
        fixture.library.root().join("unsafe"),
    ] {
        let bytes = fixture.bytes(&destination, "frames");
        let payload = SpritePayload::parse(&bytes).unwrap();
        assert!(fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || Ok(()))
            .is_err());
        assert!(!destination.exists());
    }
    for kind in ["file", "directory", "symlink"] {
        let destination = fixture.root.join(kind);
        match kind {
            "file" => fs::write(&destination, b"owned-user-data").unwrap(),
            "directory" => {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("keep"), b"owned-user-data").unwrap();
            }
            _ => std::os::unix::fs::symlink(&fixture.source, &destination).unwrap(),
        }
        let before = identity(&destination);
        let bytes = fixture.bytes(&destination, "frames");
        let payload = SpritePayload::parse(&bytes).unwrap();
        assert!(fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || Ok(()))
            .is_err());
        assert_eq!(identity(&destination), before);
        if kind == "file" {
            assert_eq!(fs::read(&destination).unwrap(), b"owned-user-data");
        }
        if kind == "directory" {
            assert_eq!(
                fs::read(destination.join("keep")).unwrap(),
                b"owned-user-data"
            );
        }
        fixture.assert_clean();
    }
}

#[test]
fn malformed_binary_lengths_geometry_png_checksums_and_tail_never_prepare_output() {
    let fixture = Fixture::new(false);
    let destination = fixture.root.join("invalid");
    let valid = fixture.bytes(&destination, "frames");
    let start = 4 + u32::from_le_bytes(valid[..4].try_into().unwrap()) as usize;
    for bytes in [
        vec![],
        vec![255; 4],
        valid[..start].to_vec(),
        {
            let mut x = valid.clone();
            x.push(1);
            x
        },
        {
            let mut x = valid.clone();
            x[start + 29] ^= 1;
            x
        },
        {
            let mut x = valid.clone();
            x[start + 12] = b'X';
            x
        },
        valid[..valid.len() - 1].to_vec(),
    ] {
        assert!(SpritePayload::parse(&bytes).is_err());
    }
    let mut header: serde_json::Value = serde_json::from_slice(&valid[4..start]).unwrap();
    for (key, value) in [
        ("fps", serde_json::json!(0)),
        ("width", serde_json::json!(2)),
        ("lengths", serde_json::json!([usize::MAX])),
        ("lengths", serde_json::json!([PNG.len(), PNG.len()])),
        ("unexpected", serde_json::json!("fake provenance")),
    ] {
        let mut changed = header.clone();
        changed[key] = value;
        let json = serde_json::to_vec(&changed).unwrap();
        let mut x = (json.len() as u32).to_le_bytes().to_vec();
        x.extend(json);
        x.extend(PNG);
        assert!(SpritePayload::parse(&x).is_err());
    }
    header["layout"]["columns"] = serde_json::json!(0);
    let json = serde_json::to_vec(&header).unwrap();
    let mut x = (json.len() as u32).to_le_bytes().to_vec();
    x.extend(json);
    x.extend(PNG);
    assert!(SpritePayload::parse(&x).is_err());
    assert!(!destination.exists());
    fixture.assert_clean();
}

#[test]
fn png_integrity_rejects_nonletter_chunk_names_even_with_correct_checksum() {
    let mut invalid = PNG[..33].to_vec();
    invalid.extend(0_u32.to_be_bytes());
    invalid.extend(b"t3XT");
    invalid.extend(crc32(b"t3XT").to_be_bytes());
    invalid.extend(&PNG[33..]);
    assert!(validate_png(&invalid, 1, 1).is_err());
}

#[test]
fn sheet_delivery_publishes_complete_frames_with_catalog_provenance_and_preserves_original() {
    let root = std::env::temp_dir().join(format!(
        "sprite-delivery-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source = root.join("Owned Sheet");
    fs::create_dir_all(&source).unwrap();
    let png = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 8, 215, 99, 248, 207, 192, 240, 31,
        0, 5, 0, 1, 255, 114, 156, 82, 103, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    fs::write(source.join("sheet.png"), png).unwrap();
    let mut lib = Library::create(&root.join("Library")).unwrap();
    lib.import_folder(&source).unwrap();
    let asset = lib.query(CatalogQuery::default()).unwrap().items.remove(0);
    let snapshot = lib.sheet_resources(asset.files[0].id).unwrap();
    let destination = root.join("delivered");
    let header = serde_json::json!({"snapshotId":snapshot.snapshot_id,"fileId":snapshot.file_id,"destination":destination,"width":1,"height":1,"layout":{"columns":1,"rows":1,"left":0,"top":0,"right":0,"bottom":0,"gapX":0,"gapY":0},"fps":10,"mode":"frames","lengths":[png.len()]});
    let json = serde_json::to_vec(&header).unwrap();
    let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
    bytes.extend(json);
    bytes.extend(png);
    let payload = SpritePayload::parse(&bytes).unwrap();
    lib.export_sprite(&snapshot, &payload, || Ok(())).unwrap();
    assert_eq!(fs::read(destination.join("frame-0001.png")).unwrap(), png);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join("sprite.json")).unwrap()).unwrap();
    assert_eq!(manifest["source"]["fileId"], snapshot.file_id);
    assert_eq!(manifest["source"]["filename"], "sheet.png");
    assert_eq!(manifest["fps"], 10);
    assert_eq!(fs::read(source.join("sheet.png")).unwrap(), png);
    assert!(lib.export_sprite(&snapshot, &payload, || Ok(())).is_err());
    assert_eq!(fs::read(destination.join("frame-0001.png")).unwrap(), png);
    drop(lib);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn packed_atlas_publishes_ordered_reconstructable_frames_with_native_provenance() {
    let original = include_bytes!("../tests/fixtures/sprite-packed-source.png");
    let atlas = include_bytes!("../tests/fixtures/sprite-packed-atlas.png");
    for mode in ["managed", "reference", "zip"] {
        let fixture = Fixture::create(mode, original);
        let destination = fixture.root.join("packed");
        let header = serde_json::json!({"snapshotId":fixture.snapshot.snapshot_id,"fileId":fixture.snapshot.file_id,"destination":destination,"width":12,"height":4,
            "layout":{"columns":3,"rows":1,"left":0,"top":0,"right":0,"bottom":0,"gapX":0,"gapY":0},"fps":10,"mode":"packed","lengths":[atlas.len()],
            "packed":{"width":11,"height":4,"frames":[[1,1,3,1,4,1],[0,0,1,2,1,1],[0,0,0,0,9,1]]}});
        let json = serde_json::to_vec(&header).unwrap();
        let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
        bytes.extend(json);
        bytes.extend(atlas);
        let payload = SpritePayload::parse(&bytes).unwrap();
        fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || Ok(()))
            .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(destination.join("sprite.json")).unwrap()).unwrap();
        assert_eq!(manifest["version"], 2);
        assert_eq!(manifest["mode"], "packed");
        assert_eq!(manifest["trimmed"], true);
        assert_eq!(manifest["rotated"], false);
        assert_eq!(manifest["extrusion"], 1);
        assert_eq!(manifest["coordinateOrigin"], "topLeft");
        assert_eq!(manifest["frames"].as_array().unwrap().len(), 3);
        assert_eq!(
            manifest["frames"][0]["trim"],
            serde_json::json!({"x":1,"y":1,"width":3,"height":1})
        );
        assert_eq!(
            manifest["frames"][0]["sourceSize"],
            serde_json::json!({"width":4,"height":4})
        );
        assert_eq!(manifest["frames"][0]["durationMs"], 100.0);
        assert_eq!(manifest["frames"][0]["index"], 0);
        assert_eq!(
            manifest["frames"][1]["source"],
            serde_json::json!({"x":4,"y":0,"width":4,"height":4})
        );
        assert_eq!(manifest["frames"][2]["empty"], true);
        assert_eq!(manifest["source"]["fileId"], fixture.snapshot.file_id);
        assert_eq!(fs::read(destination.join("atlas.png")).unwrap(), atlas);
        assert_eq!(
            fs::read(
                fixture
                    .library
                    .resolve_preview(fixture.snapshot.file_id)
                    .unwrap()
            )
            .unwrap(),
            original
        );
        assert_eq!(
            fs::read(fixture.source.join("nested/sheet.png")).unwrap(),
            original
        );
    }
}

fn packed_bytes(
    fixture: &Fixture,
    destination: &Path,
    alter: impl FnOnce(&mut serde_json::Value),
) -> Vec<u8> {
    let atlas = include_bytes!("../tests/fixtures/sprite-packed-atlas.png");
    let mut header = serde_json::json!({"snapshotId":fixture.snapshot.snapshot_id,"fileId":fixture.snapshot.file_id,"destination":destination,"width":12,"height":4,
        "layout":{"columns":3,"rows":1,"left":0,"top":0,"right":0,"bottom":0,"gapX":0,"gapY":0},"fps":10,"mode":"packed","lengths":[atlas.len()],
        "packed":{"width":11,"height":4,"frames":[[1,1,3,1,4,1],[0,0,1,2,1,1],[0,0,0,0,9,1]]}});
    alter(&mut header);
    let json = serde_json::to_vec(&header).unwrap();
    let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
    bytes.extend(json);
    bytes.extend(atlas);
    bytes
}

#[test]
fn invalid_packed_rectangles_and_envelopes_cannot_prepare_an_export() {
    let fixture = Fixture::create(
        "managed",
        include_bytes!("../tests/fixtures/sprite-packed-source.png"),
    );
    let destination = fixture.root.join("invalid");
    let invalid = [
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,3,1,4,1],[0,0,1,2,1,1]]}),
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,4,1,4,1],[0,0,1,2,1,1],[0,0,0,0,9,1]]}),
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,3,1,4,1],[0,0,1,2,3,1],[0,0,0,0,9,1]]}),
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,3,1,4,1],[0,0,1,2,1,1],[0,0,0,1,9,1]]}),
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,3,1,4,1],[0,0,1,2,1,1],[1,0,0,0,9,1]]}),
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,3,1,0,1],[0,0,1,2,1,1],[0,0,0,0,9,1]]}),
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,3,1,4,1],[0,0,1,2,1,1],[0,0,0,0,10,1]]}),
        serde_json::json!({"width":10,"height":4,"frames":[[1,1,3,1,4,1],[0,0,1,2,1,1],[0,0,0,0,8,1]]}),
        serde_json::json!({"width":8192,"height":8192,"frames":[[1,1,3,1,4,1],[0,0,1,2,1,1],[0,0,0,0,9,1]]}),
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,3,1,4.5,1],[0,0,1,2,1,1],[0,0,0,0,9,1]]}),
        serde_json::json!({"width":11,"height":4,"frames":[[1,1,3,1,-1,1],[0,0,1,2,1,1],[0,0,0,0,9,1]]}),
        serde_json::Value::Null,
    ];
    for plan in invalid {
        let bytes = packed_bytes(&fixture, &destination, |header| header["packed"] = plan);
        assert!(SpritePayload::parse(&bytes).is_err());
        assert!(!destination.exists());
    }
    for mode in ["frames", "atlas"] {
        assert!(
            SpritePayload::parse(&packed_bytes(&fixture, &destination, |header| header
                ["mode"] =
                serde_json::json!(mode)))
            .is_err()
        );
    }
    assert_eq!(
        fs::read(fixture.source.join("nested/sheet.png")).unwrap(),
        include_bytes!("../tests/fixtures/sprite-packed-source.png")
    );
}

#[test]
fn packed_export_cancellation_collision_and_staging_failures_preserve_originals_and_recover() {
    for mode in ["managed", "reference", "zip"] {
        let fixture = Fixture::create(
            mode,
            include_bytes!("../tests/fixtures/sprite-packed-source.png"),
        );
        for phase in ["write", "cancel"] {
            let destination = fixture.root.join(phase);
            let bytes = packed_bytes(&fixture, &destination, |_| {});
            let payload = SpritePayload::parse(&bytes).unwrap();
            if phase != "cancel" {
                TEST_FAILURE.with(|p| p.set(Some(phase)));
            }
            assert!(fixture
                .library
                .export_sprite(&fixture.snapshot, &payload, || if phase == "cancel" {
                    Err("cancelled".into())
                } else {
                    Ok(())
                })
                .is_err());
            TEST_FAILURE.with(|p| p.set(None));
            assert!(!destination.exists());
            assert!(!fs::read_dir(&fixture.root).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".game-assets-sprite-")));
            fixture
                .library
                .export_sprite(&fixture.snapshot, &payload, || Ok(()))
                .unwrap();
            let before = fs::read(destination.join("sprite.json")).unwrap();
            assert!(fixture
                .library
                .export_sprite(&fixture.snapshot, &payload, || Ok(()))
                .is_err());
            assert_eq!(fs::read(destination.join("sprite.json")).unwrap(), before);
        }
        let destination = fixture.root.join("trash");
        let bytes = packed_bytes(&fixture, &destination, |_| {});
        let payload = SpritePayload::parse(&bytes).unwrap();
        Library::open(fixture.library.root())
            .unwrap()
            .trash_file(fixture.snapshot.file_id)
            .unwrap();
        assert!(fixture
            .library
            .export_sprite(&fixture.snapshot, &payload, || Ok(()))
            .is_err());
        assert!(!destination.exists());
        assert_eq!(
            fs::read(fixture.source.join("nested/sheet.png")).unwrap(),
            include_bytes!("../tests/fixtures/sprite-packed-source.png")
        );
    }
}

#[test]
fn packed_maximum_timeline_has_a_bounded_plan_and_keeps_every_empty_frame() {
    let fixture = Fixture::create(
        "managed",
        include_bytes!("../tests/fixtures/sprite-packed-empty-source.png"),
    );
    let atlas = include_bytes!("../tests/fixtures/sprite-packed-empty-atlas.png");
    let destination = fixture.root.join("maximum");
    let coordinates = (0..256)
        .map(|i| [0, 0, 0, 0, 1 + (i % 16) * 3, 1 + (i / 16) * 3])
        .collect::<Vec<_>>();
    let header = serde_json::json!({"snapshotId":fixture.snapshot.snapshot_id,"fileId":fixture.snapshot.file_id,"destination":destination,"width":16,"height":16,
        "layout":{"columns":16,"rows":16,"left":0,"top":0,"right":0,"bottom":0,"gapX":0,"gapY":0},"fps":60,"mode":"packed","lengths":[atlas.len()],
        "packed":{"width":48,"height":48,"frames":coordinates}});
    let json = serde_json::to_vec(&header).unwrap();
    assert!(json.len() < 16 * 1024);
    let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
    bytes.extend(json);
    bytes.extend(atlas);
    let payload = SpritePayload::parse(&bytes).unwrap();
    fixture
        .library
        .export_sprite(&fixture.snapshot, &payload, || Ok(()))
        .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join("sprite.json")).unwrap()).unwrap();
    let frames = manifest["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 256);
    assert!(frames.iter().all(|f| f["empty"] == true));
    assert_eq!(frames[255]["index"], 255);
    assert_eq!(
        frames[255]["source"],
        serde_json::json!({"x":15,"y":15,"width":1,"height":1})
    );
    assert_eq!(
        frames[255]["output"],
        serde_json::json!({"x":46,"y":46,"width":1,"height":1})
    );
    assert_eq!(fs::read(destination.join("atlas.png")).unwrap(), atlas);
}
