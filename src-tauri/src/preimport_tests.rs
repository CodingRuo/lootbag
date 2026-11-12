use super::*;
use crate::library::Library;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "preimport-owned-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn folder_preparation_accepts_owned_actual_catalog_and_incoming_files_without_writes() {
    let w = Workspace::new();
    let old = w.0.join("Old");
    let incoming = w.0.join("Incoming");
    fs::create_dir(&old).unwrap();
    fs::create_dir(&incoming).unwrap();
    fs::write(old.join("First.wav"), b"abc").unwrap();
    fs::write(incoming.join("Renamed.wav"), b"abc").unwrap();
    let mut library = Library::create(&w.0.join("Library")).unwrap();
    library.import_folder(&old).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let prepared = prepare(&root, &incoming, false, &ImportControl::default(), |_| {}).unwrap();
    assert!(prepared.verified(), "{:?}", prepared.evidence.issues);
    assert_eq!(prepared.evidence.status, "complete");
    assert_eq!(prepared.evidence.duplicate_incoming_files, 1);
    assert_eq!(
        prepared.evidence.matches[0].incoming_relative_path,
        "Renamed.wav"
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
}

#[test]
fn verified_folder_copies_every_companion_and_empty_directory_through_the_existing_catalog() {
    let w = Workspace::new();
    let incoming = w.0.join("Incoming");
    fs::create_dir(&incoming).unwrap();
    fs::create_dir(incoming.join("Empty")).unwrap();
    for (n, b) in [
        ("Model.obj", b"abc".as_slice()),
        ("Model.mtl", b"companion"),
        ("LICENSE.txt", b"license"),
        ("._Model.obj", b"metadata"),
    ] {
        fs::write(incoming.join(n), b).unwrap();
    }
    let library = Library::create(&w.0.join("Library")).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let control = ImportControl::default();
    let prepared = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    assert!(prepared.verified());
    let package = consume(
        &root,
        prepared,
        false,
        &control,
        &mut |_| {},
        &mut |operation| operation(),
    )
    .unwrap();
    assert_eq!(package.asset_count, 1);
    let dest = root.join(&package.relative_path);
    for n in ["Model.obj", "Model.mtl", "LICENSE.txt", "._Model.obj"] {
        assert_eq!(
            fs::read(dest.join(n)).unwrap(),
            fs::read(incoming.join(n)).unwrap()
        )
    }
    assert!(dest.join("Empty").is_dir());
}
#[test]
fn zip_initial_preparation_is_unavailable_without_constructing_an_archive_or_writing() {
    let w = Workspace::new();
    let file = w.0.join("Broken.zip");
    fs::write(&file, b"not a zip").unwrap();
    let library = Library::create(&w.0.join("Library")).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let control = ImportControl::default();
    let p = prepare(&root, &file, true, &control, |_| {
        panic!("ZIP contents must not be inspected")
    })
    .unwrap();
    assert!(!p.verified());
    assert_eq!(p.evidence.status, "unavailable");
    assert_eq!(p.evidence.incoming_files, None);
    assert!(
        consume(&root, p, false, &control, &mut |_| {}, &mut |f| f())
            .unwrap_err()
            .contains("Bestätigung")
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert!(!root.join("packages").exists());
}
fn incoming_fixture(w: &Workspace) -> (PathBuf, PathBuf) {
    let incoming = w.0.join("Incoming");
    fs::create_dir(&incoming).unwrap();
    fs::write(incoming.join("One.wav"), b"abc").unwrap();
    let library = Library::create(&w.0.join("Library")).unwrap();
    let root = library.root().to_owned();
    drop(library);
    (root, incoming)
}
#[test]
fn final_publisher_rejects_a_same_size_source_change_after_copy_validation() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let control = ImportControl::default();
    let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    let modified = fs::metadata(incoming.join("One.wav"))
        .unwrap()
        .modified()
        .unwrap();
    let result = consume(&root, p, false, &control, &mut |_| {}, &mut |operation| {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(incoming.join("One.wav"))
            .unwrap();
        fs::write(incoming.join("One.wav"), b"abd").unwrap();
        file.set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        operation()
    });
    assert!(
        result.is_err(),
        "final source change must require reprepare"
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert!(fs::read_dir(root.join("packages"))
        .unwrap()
        .next()
        .is_none());
}
#[test]
fn final_publisher_rejects_staging_tampering_after_the_copied_stream_check() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let control = ImportControl::default();
    let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    let result = consume(&root, p, false, &control, &mut |_| {}, &mut |operation| {
        let stage = fs::read_dir(root.join("packages"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(stage.join("One.wav"), b"abd").unwrap();
        operation()
    });
    assert!(result.is_err(), "tampered staging must never publish");
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert!(fs::read_dir(root.join("packages"))
        .unwrap()
        .next()
        .is_none());
}
#[test]
fn changed_source_file_ancestor_and_companion_topology_require_reprepare_before_any_copy() {
    for kind in ["replace", "mtime", "ancestor", "companion", "nested"] {
        let w = Workspace::new();
        let (root, incoming) = incoming_fixture(&w);
        fs::create_dir(incoming.join("Nested")).unwrap();
        fs::write(incoming.join("Nested/LICENSE.txt"), b"license").unwrap();
        let before = fs::read(root.join("catalog.sqlite3")).unwrap();
        let control = ImportControl::default();
        let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
        assert!(p.verified());
        match kind {
            "replace" => {
                fs::rename(incoming.join("One.wav"), w.0.join("old.wav")).unwrap();
                fs::write(incoming.join("One.wav"), b"abc").unwrap();
            }
            "mtime" => {
                let file = fs::OpenOptions::new()
                    .write(true)
                    .open(incoming.join("One.wav"))
                    .unwrap();
                let time = file.metadata().unwrap().modified().unwrap();
                fs::write(incoming.join("One.wav"), b"abd").unwrap();
                file.set_times(fs::FileTimes::new().set_modified(time))
                    .unwrap();
            }
            "ancestor" => {
                let moved = w.0.with_extension("held-original");
                fs::rename(&w.0, &moved).unwrap();
                fs::create_dir(&w.0).unwrap();
                fs::rename(moved.join("Incoming"), &incoming).unwrap();
                fs::rename(moved.join("Library"), &root).unwrap();
                fs::remove_dir(&moved).unwrap();
            }
            "companion" => {
                fs::write(incoming.join("Nested/LICENSE.txt"), b"changed").unwrap();
            }
            _ => {
                fs::write(incoming.join("Nested/New.txt"), b"new").unwrap();
            }
        }
        let result = consume(&root, p, false, &control, &mut |_| {}, &mut |f| f());
        assert!(result.is_err(), "{kind}");
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
        assert!(!root.join("packages").exists(), "{kind}");
    }
}
#[test]
fn active_existing_variants_include_all_pages_and_externals_but_exclude_trash() {
    let w = Workspace::new();
    let old = w.0.join("Old");
    let external = w.0.join("External");
    let incoming = w.0.join("Incoming");
    for p in [&old, &external, &incoming] {
        fs::create_dir(p).unwrap();
    }
    for i in 0..85 {
        fs::write(old.join(format!("P{i:03}.wav")), format!("{i:03}")).unwrap();
    }
    fs::write(external.join("External.obj"), b"084").unwrap();
    fs::write(incoming.join("Renamed.wav"), b"084").unwrap();
    fs::write(incoming.join("Unequal.wav"), b"085").unwrap();
    let mut library = Library::create(&w.0.join("Library")).unwrap();
    let package = library.import_folder(&old).unwrap();
    library.reference_folder(&external).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let control = ImportControl::default();
    let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    assert!(p.verified(), "{:?}", p.evidence.issues);
    assert_eq!(p.evidence.existing_variants, Some(86));
    assert_eq!(p.evidence.duplicate_incoming_files, 1);
    assert_eq!(p.evidence.matches.len(), 2);
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    let mut library = Library::open(&root).unwrap();
    library.trash_package(package.id).unwrap();
    drop(library);
    let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    assert_eq!(p.evidence.existing_variants, Some(1));
    assert_eq!(p.evidence.matches.len(), 1);
    assert_eq!(
        p.evidence.matches[0].existing_original_filename,
        "External.obj"
    );
}
#[test]
fn unavailable_existing_sources_are_honest_incomplete_evidence_and_whole_package_fallback() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let old = w.0.join("Old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("Missing.wav"), b"abc").unwrap();
    let mut library = Library::open(&root).unwrap();
    let oldpackage = library.import_folder(&old).unwrap();
    fs::remove_file(root.join(&oldpackage.relative_path).join("Missing.wav")).unwrap();
    drop(library);
    let control = ImportControl::default();
    let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    assert!(!p.verified());
    assert_eq!(p.evidence.status, "incomplete");
    assert_eq!(p.evidence.existing_variants, Some(1));
    assert_eq!(p.evidence.unavailable_existing_variants, 1);
    assert_eq!(p.evidence.unavailable_candidates, 0);
    let package = consume(&root, p, true, &control, &mut |_| {}, &mut |f| f()).unwrap();
    assert_eq!(
        fs::read(root.join(package.relative_path).join("One.wav")).unwrap(),
        b"abc"
    );
}
#[test]
fn inspection_limits_allow_explicit_unverified_whole_package_continuation() {
    for limits in [
        Limits {
            entries: 0,
            ..LIMITS
        },
        Limits { file: 2, ..LIMITS },
        Limits { total: 2, ..LIMITS },
        Limits {
            retained: 1,
            ..LIMITS
        },
        Limits { reads: 2, ..LIMITS },
        Limits {
            work: Duration::ZERO,
            ..LIMITS
        },
    ] {
        let w = Workspace::new();
        let (root, incoming) = incoming_fixture(&w);
        let control = ImportControl::default();
        let prepared = prepare_with_limits(&root, &incoming, false, &control, |_| {}, limits);
        let p = prepared.unwrap();
        assert!(!p.verified());
        assert_eq!(p.evidence.status, "incomplete");
        assert!(!p.evidence.issues.is_empty());
        let package = consume(&root, p, true, &control, &mut |_| {}, &mut |f| f()).unwrap();
        assert_eq!(
            fs::read(root.join(package.relative_path).join("One.wav")).unwrap(),
            b"abc"
        );
    }
}
#[test]
fn incoming_sparse_file_above_inspection_limit_is_not_a_preparation_import_ban() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    fs::File::create(incoming.join("Big.wav"))
        .unwrap()
        .set_len(LIMITS.file + 1)
        .unwrap();
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let p = prepare(&root, &incoming, false, &ImportControl::default(), |_| {}).unwrap();
    assert!(!p.verified());
    assert_eq!(p.evidence.status, "incomplete");
    assert!(!p.evidence.issues.is_empty());
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert!(!root.join("packages").exists());
}
#[test]
fn cancellation_before_copy_and_during_copy_preserves_original_images() {
    for during in [false, true] {
        let w = Workspace::new();
        let (root, incoming) = incoming_fixture(&w);
        fs::write(incoming.join("One.wav"), vec![b'a'; 131072]).unwrap();
        let control = ImportControl::default();
        let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
        let before = fs::read(root.join("catalog.sqlite3")).unwrap();
        if !during {
            control.cancel()
        }
        let mut stopped = false;
        let result = consume(
            &root,
            p,
            false,
            &control,
            &mut |progress| {
                if during && progress.completed_bytes > 0 && !stopped {
                    stopped = true;
                    control.cancel()
                }
            },
            &mut |f| f(),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
        if during {
            assert!(stopped);
            assert!(fs::read_dir(root.join("packages"))
                .unwrap()
                .next()
                .is_none());
        } else {
            assert!(!root.join("packages").exists())
        }
    }
}
#[test]
fn owned_stage_replacement_does_not_delete_unrelated_replacement_data() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let control = ImportControl::default();
    let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let mut changed = false;
    let mut replacement = None;
    let result = consume(
        &root,
        p,
        false,
        &control,
        &mut |progress| {
            if progress.completed_bytes > 0 && !changed {
                changed = true;
                let stage = fs::read_dir(root.join("packages"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::rename(&stage, w.0.join("held-owned-stage")).unwrap();
                fs::create_dir(&stage).unwrap();
                fs::write(stage.join("Unrelated.txt"), b"keep").unwrap();
                replacement = Some(stage);
            }
        },
        &mut |f| f(),
    );
    assert!(result.is_err());
    assert_eq!(
        fs::read(replacement.unwrap().join("Unrelated.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
}
#[test]
fn explicit_zip_authorization_estimates_then_preserves_complete_legacy_package() {
    use std::io::Write;
    let w = Workspace::new();
    let zip = w.0.join("Owned.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    for (name, bytes) in [
        ("Nested/Renamed.wav", b"abc".as_slice()),
        ("Nested/LICENSE.txt", b"license"),
    ] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    let library = Library::create(&w.0.join("Library")).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let control = ImportControl::default();
    let p = prepare(&root, &zip, true, &control, |_| {}).unwrap();
    let estimate = authorize(&p, &root, &control).unwrap().unwrap();
    assert_eq!(estimate.entry_count, 2);
    assert_eq!(estimate.uncompressed_bytes, 10);
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    let package = consume(&root, p, true, &control, &mut |_| {}, &mut |f| f()).unwrap();
    assert_eq!(
        fs::read(root.join(&package.relative_path).join("Nested/Renamed.wav")).unwrap(),
        b"abc"
    );
    assert_eq!(
        fs::read(root.join(package.relative_path).join("Nested/LICENSE.txt")).unwrap(),
        b"license"
    );
}
#[test]
fn catalogue_change_after_prepare_requires_reprepare_and_keeps_previous_packages() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let mut library = Library::open(&root).unwrap();
    let package = library.import_folder(&incoming).unwrap();
    let asset = library
        .query(crate::library::CatalogQuery::default())
        .unwrap()
        .items[0]
        .id;
    drop(library);
    let control = ImportControl::default();
    let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    let mut library = Library::open(&root).unwrap();
    library
        .annotate_asset(asset, "Keep this annotation", &["keep".into()])
        .unwrap();
    drop(library);
    let writer_image = fs::read(root.join("catalog.sqlite3")).unwrap();
    assert!(consume(&root, p, false, &control, &mut |_| {}, &mut |f| f()).is_err());
    assert_eq!(
        fs::read(root.join("catalog.sqlite3")).unwrap(),
        writer_image
    );
    assert_eq!(
        fs::read(root.join(package.relative_path).join("One.wav")).unwrap(),
        b"abc"
    );
    assert_eq!(fs::read_dir(root.join("packages")).unwrap().count(), 1);
}
#[test]
fn private_publisher_reserves_sqlite_before_cataloguing_and_cancel_rolls_back_owned_package() {
    for cancel in [false, true] {
        let w = Workspace::new();
        let (root, incoming) = incoming_fixture(&w);
        let before = fs::read(root.join("catalog.sqlite3")).unwrap();
        let control = ImportControl::default();
        let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
        let other = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
        other.busy_timeout(Duration::ZERO).unwrap();
        let mut locked = false;
        let result = consume(
            &root,
            p,
            false,
            &control,
            &mut |progress| {
                if progress.stage == "cataloging" && !locked {
                    locked = true;
                    assert!(other
                        .execute("UPDATE packages SET name='Unexpected writer'", [])
                        .is_err());
                    if cancel {
                        control.cancel()
                    }
                }
            },
            &mut |f| f(),
        );
        assert!(locked);
        if cancel {
            assert!(result.is_err());
            assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
            assert!(fs::read_dir(root.join("packages"))
                .unwrap()
                .next()
                .is_none());
        } else {
            assert!(result.is_ok());
        }
    }
}
#[test]
fn prepare_wal_catalog_is_explicitly_incomplete_and_preserves_catalogue_sidecars() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let connection = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection
        .execute("UPDATE packages SET name=name", [])
        .unwrap();
    let image = |name: &str| fs::read(root.join(name)).ok();
    let before = [
        image("catalog.sqlite3"),
        image("catalog.sqlite3-wal"),
        image("catalog.sqlite3-shm"),
    ];
    let p = prepare(&root, &incoming, false, &ImportControl::default(), |_| {}).unwrap();
    assert!(!p.verified());
    assert_eq!(p.evidence.status, "incomplete");
    assert_eq!(p.evidence.existing_variants, None);
    assert!(!p.evidence.issues.is_empty());
    assert_eq!(
        [
            image("catalog.sqlite3"),
            image("catalog.sqlite3-wal"),
            image("catalog.sqlite3-shm")
        ],
        before
    );
    drop(connection);
}
#[test]
fn matches_are_bounded_with_exact_total_counts_and_originals_never_omitted() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    for n in 0..34 {
        fs::write(incoming.join(format!("Match{n}.wav")), b"abc").unwrap();
    }
    let old = w.0.join("Old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("Original.wav"), b"abc").unwrap();
    let mut library = Library::open(&root).unwrap();
    library.import_folder(&old).unwrap();
    drop(library);
    let control = ImportControl::default();
    let p = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    assert!(p.verified());
    assert_eq!(p.evidence.matches.len(), 30);
    assert!(p.evidence.details_truncated);
    assert_eq!(p.evidence.duplicate_incoming_files, 35);
    assert_eq!(p.evidence.examined_candidates, 35);
    let package = consume(&root, p, false, &control, &mut |_| {}, &mut |f| f()).unwrap();
    assert_eq!(package.asset_count, 35);
    assert_eq!(
        fs::read_dir(root.join(package.relative_path))
            .unwrap()
            .count(),
        35
    );
}
#[test]
fn stop_during_preparation_is_bounded_and_read_only_and_expired_inspection_remains_reviewable() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    fs::write(incoming.join("One.wav"), vec![b'a'; 131072]).unwrap();
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let control = ImportControl::default();
    let result = prepare(&root, &incoming, false, &control, |progress| {
        if progress.stage == "inventory" {
            control.cancel()
        }
    });
    assert!(result.is_err());
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert!(!root.join("packages").exists());
    let p = prepare_with_limits(
        &root,
        &incoming,
        false,
        &ImportControl::default(),
        |_| {},
        Limits {
            work: Duration::ZERO,
            ..LIMITS
        },
    )
    .unwrap();
    assert!(!p.verified());
    assert_eq!(p.evidence.status, "incomplete");
    assert!(!p.evidence.issues.is_empty());
}
#[test]
fn unverified_destination_replacement_requires_reprepare_before_copy() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let control = ImportControl::default();
    let p = prepare_with_limits(
        &root,
        &incoming,
        false,
        &control,
        |_| {},
        Limits { file: 2, ..LIMITS },
    )
    .unwrap();
    assert!(!p.verified());
    fs::rename(&root, w.0.join("OriginalLibrary")).unwrap();
    let replacement = Library::create(&root).unwrap();
    drop(replacement);
    let replacement_image = fs::read(root.join("catalog.sqlite3")).unwrap();
    let result = consume(&root, p, true, &control, &mut |_| {}, &mut |f| f());
    assert!(
        result.is_err(),
        "token must bind the original destination directory"
    );
    assert_eq!(
        fs::read(root.join("catalog.sqlite3")).unwrap(),
        replacement_image
    );
    assert!(!root.join("packages").exists());
}
#[test]
fn unverified_legacy_failure_preserves_unrelated_replacement_stage() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let control = ImportControl::default();
    let p = prepare_with_limits(
        &root,
        &incoming,
        false,
        &control,
        |_| {},
        Limits { file: 2, ..LIMITS },
    )
    .unwrap();
    let mut changed = false;
    let mut replacement = None;
    let result = consume(
        &root,
        p,
        true,
        &control,
        &mut |progress| {
            if progress.completed_bytes > 0 && !changed {
                changed = true;
                let stage = fs::read_dir(root.join("packages"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::rename(&stage, w.0.join("OriginalStage")).unwrap();
                fs::create_dir(&stage).unwrap();
                fs::write(stage.join("Unrelated.txt"), b"keep").unwrap();
                replacement = Some(stage);
                control.cancel();
            }
        },
        &mut |f| f(),
    );
    assert!(result.is_err());
    assert!(control.cleanup_failed());
    assert_eq!(
        fs::read(replacement.unwrap().join("Unrelated.txt")).unwrap(),
        b"keep"
    );
}

#[test]
fn preimport_independent_catalog_writer_child() {
    let Ok(path) = std::env::var("PREIMPORT_OWNED_WRITER_CATALOG") else {
        return;
    };
    let path = PathBuf::from(path);
    assert!(path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("preimport-owned-"));
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.busy_timeout(Duration::ZERO).unwrap();
    match connection.execute_batch("UPDATE marker SET value='writer';") {
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::DatabaseBusy =>
        {
            println!("OWNED_WRITER_BUSY")
        }
        other => panic!("independent writer must remain excluded: {other:?}"),
    }
}

fn assert_independent_writer_excluded(root: &Path) {
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "library::preimport::tests::preimport_independent_catalog_writer_child",
            "--nocapture",
        ])
        .env(
            "PREIMPORT_OWNED_WRITER_CATALOG",
            root.join("catalog.sqlite3"),
        )
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(String::from_utf8_lossy(&child.stdout).contains("OWNED_WRITER_BUSY"));
}

#[test]
fn preimport_reserved_publisher_excludes_independent_writer_after_final_metadata_checks() {
    for aliases in [false, true] {
        let w = Workspace::new();
        let (root, incoming) = incoming_fixture(&w);
        let mut library = Library::open(&root).unwrap();
        library
            .connection
            .execute_batch(
                "CREATE TABLE marker(value TEXT); INSERT INTO marker VALUES('original');",
            )
            .unwrap();
        if aliases {
            let old = w.0.join("Old");
            fs::create_dir(&old).unwrap();
            fs::write(old.join("Old.wav"), b"old").unwrap();
            let package = library.import_folder(&old).unwrap();
            let length = fs::metadata(root.join("catalog.sqlite3")).unwrap().len();
            library
                .connection
                .execute("UPDATE asset_files SET size_bytes=?1", [length as i64])
                .unwrap();
            let indexed = root.join(package.relative_path).join("Old.wav");
            fs::remove_file(&indexed).unwrap();
            fs::hard_link(root.join("catalog.sqlite3"), indexed).unwrap();
            fs::hard_link(root.join("catalog.sqlite3"), root.join("catalog-alias.txt")).unwrap();
        }
        drop(library);
        let before = fs::read(root.join("catalog.sqlite3")).unwrap();
        let control = ImportControl::default();
        let mut prepared = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
        assert!(prepared.verified(), "{:?}", prepared.evidence.issues);
        prepared
            .folder
            .as_mut()
            .unwrap()
            .existing
            .reader
            .reserve_publication()
            .unwrap();
        let folder = prepared.folder.as_ref().unwrap();
        let mut library =
            Library::connect(&root, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
        let mut work = Work {
            control: &control,
            started: Instant::now(),
            limits: LIMITS,
            charged: 0,
            bytes: 0,
            examined: 0,
            total: None,
            progress: |_| {},
            stage: "",
            last_progress: Instant::now(),
        };
        let stage = Stage::create(folder, &work).unwrap();
        let source = prepared.pin.validate().unwrap();
        let mut progress = |_| {};
        let mut run = super::super::imports::ImportRun::new(&control, &mut progress);
        let result = library.save_import_with(
            "MustNotPublish".into(),
            incoming.to_str().unwrap().into(),
            "packages/no-publish".into(),
            vec![],
            &mut run,
            true,
            || {
                prepared.pin.validate()?;
                prepared.destination.validate(&root)?;
                stage.validate(&folder.existing, &work)?;
                inventory(&source, &mut work)?;
                inventory(&stage.directory, &mut work)?;
                assert_independent_writer_excluded(&root);
                // A replaced output alias is measured without closing SQLite's inode.
                fs::hard_link(
                    root.join("catalog.sqlite3"),
                    root.join("packages")
                        .join(&stage.name)
                        .join("late-alias.txt"),
                )
                .unwrap();
                inventory(&stage.directory, &mut work)?;
                assert!(stage.validate(&folder.existing, &work).is_err());
                assert_independent_writer_excluded(&root);
                Err("owned test stops before publication".into())
            },
        );
        assert!(result.is_err());
        fs::remove_file(
            root.join("packages")
                .join(&stage.name)
                .join("late-alias.txt"),
        )
        .unwrap();
        assert!(stage.cleanup());
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
        assert_eq!(fs::read(incoming.join("One.wav")).unwrap(), b"abc");
        assert!(!root.join("packages/no-publish").exists());
        if aliases {
            drop(library);
            let error =
                consume(&root, prepared, false, &control, &mut |_| {}, &mut |f| f()).unwrap_err();
            assert!(error.contains("bestehende Variante"));
            assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
        }
    }
}

#[test]
fn preimport_cancel_during_stage_construction_cleans_owned_stage() {
    for replace in [false, true] {
        let w = Workspace::new();
        let (root, incoming) = incoming_fixture(&w);
        for i in 0..600 {
            fs::write(root.join(format!("retained-{i:04}.txt")), b"retained").unwrap();
        }
        let control = std::sync::Arc::new(ImportControl::default());
        let prepared = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
        let before = fs::read(root.join("catalog.sqlite3")).unwrap();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let root = root.clone();
            let control = control.clone();
            let done = done.clone();
            let saw = saw.clone();
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    let stage = fs::read_dir(root.join("packages"))
                        .ok()
                        .and_then(|entries| {
                            entries.flatten().find(|entry| {
                                entry
                                    .file_name()
                                    .to_str()
                                    .is_some_and(|name| name.starts_with(".import-"))
                            })
                        });
                    if let Some(stage) = stage {
                        if replace {
                            fs::rename(stage.path(), root.join("MovedOwnedStage")).unwrap();
                            fs::create_dir(stage.path()).unwrap();
                            fs::write(stage.path().join("keep.txt"), b"unrelated replacement")
                                .unwrap();
                        }
                        saw.store(true, Ordering::Relaxed);
                        control.cancel();
                        break;
                    }
                    std::thread::yield_now();
                }
            })
        };
        let result = consume(&root, prepared, false, &control, &mut |_| {}, &mut |f| f());
        done.store(true, Ordering::Relaxed);
        watcher.join().unwrap();
        assert!(saw.load(Ordering::Relaxed));
        assert!(result.is_err());
        if replace {
            let replacement = fs::read_dir(root.join("packages"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            assert_eq!(
                fs::read(replacement.join("keep.txt")).unwrap(),
                b"unrelated replacement"
            );
            assert!(control.cleanup_failed());
            assert!(result.unwrap_err().contains("nicht vollständig entfernt"));
        } else {
            assert_eq!(fs::read_dir(root.join("packages")).unwrap().count(), 0);
            assert!(!control.cleanup_failed());
        }
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
        assert_eq!(fs::read(incoming.join("One.wav")).unwrap(), b"abc");
    }
}

#[test]
fn preimport_overlapping_catalog_reader_must_finish_before_publication_reservation() {
    let w = Workspace::new();
    let (root, incoming) = incoming_fixture(&w);
    let library = Library::open(&root).unwrap();
    library
        .connection
        .execute_batch("CREATE TABLE marker(value TEXT); INSERT INTO marker VALUES('original');")
        .unwrap();
    drop(library);
    let control = ImportControl::default();
    let mut prepared = prepare(&root, &incoming, false, &control, |_| {}).unwrap();
    let overlapping = super::super::storage::CatalogReader::open(&root).unwrap();
    assert!(prepared
        .folder
        .as_mut()
        .unwrap()
        .existing
        .reader
        .reserve_publication()
        .is_err());
    drop(overlapping);
    prepared
        .folder
        .as_mut()
        .unwrap()
        .existing
        .reader
        .reserve_publication()
        .unwrap();
    let mut writer = Library::connect(&root, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
    let mut progress = |_| {};
    let mut run = super::super::imports::ImportRun::new(&control, &mut progress);
    let result = writer.save_import_with(
        "No publication".into(),
        incoming.to_str().unwrap().into(),
        "packages/no-publish".into(),
        vec![],
        &mut run,
        true,
        || {
            assert!(super::super::storage::CatalogReader::open(&root).is_err());
            assert_independent_writer_excluded(&root);
            Err("stop before publication".into())
        },
    );
    assert!(result.is_err());
}

#[test]
fn preimport_incoming_catalog_and_sidecar_aliases_preserve_every_original() {
    for (alias, limited) in [
        ("catalog.sqlite3", false),
        ("catalog.sqlite3", true),
        ("catalog.sqlite3-journal", false),
        ("catalog.sqlite3-journal", true),
        ("catalog.sqlite3-wal", false),
        ("catalog.sqlite3-wal", true),
        ("catalog.sqlite3-shm", false),
        ("catalog.sqlite3-shm", true),
    ] {
        let w = Workspace::new();
        let (root, incoming) = incoming_fixture(&w);
        let sqlite = if alias.ends_with("journal")
            || alias.ends_with("wal")
            || alias.ends_with("shm")
        {
            let connection = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
            let mode = if alias.ends_with("journal") {
                "PERSIST"
            } else {
                "WAL"
            };
            connection
                .pragma_update(None, "journal_mode", mode)
                .unwrap();
            connection.execute_batch("CREATE TABLE owned_sidecar_fixture(value TEXT); INSERT INTO owned_sidecar_fixture VALUES('original');").unwrap();
            if mode == "WAL" {
                Some(connection)
            } else {
                drop(connection);
                None
            }
        } else {
            None
        };
        let _keep_wal_writer_alive = sqlite;
        fs::hard_link(root.join(alias), incoming.join("LICENSE.txt")).unwrap();
        let catalogue_before = fs::read(root.join("catalog.sqlite3")).unwrap();
        let source_before = fs::read(incoming.join("LICENSE.txt")).unwrap();
        let control = ImportControl::default();
        let result = if limited {
            let limits = Limits {
                entries: 0,
                ..LIMITS
            };
            let prepared =
                prepare_with_limits(&root, &incoming, false, &control, |_| {}, limits).unwrap();
            assert!(!prepared.verified());
            consume(&root, prepared, true, &control, &mut |_| {}, &mut |f| f())
        } else {
            prepare(&root, &incoming, false, &control, |_| {}).and_then(|prepared| {
                consume(&root, prepared, true, &control, &mut |_| {}, &mut |f| f())
            })
        };
        assert!(result.is_err(),"catalogue/sidecar alias must be refused even as an ordinary companion or unverified fallback");
        assert_eq!(
            fs::read(root.join("catalog.sqlite3")).unwrap(),
            catalogue_before
        );
        assert_eq!(
            fs::read(incoming.join("LICENSE.txt")).unwrap(),
            source_before
        );
        assert_eq!(fs::read(incoming.join("One.wav")).unwrap(), b"abc");
        assert!(fs::read_dir(root.join("packages"))
            .ok()
            .is_none_or(|mut e| e.next().is_none()));
    }
}

#[test]
fn preimport_anchored_metadata_preserves_full_stamps_without_following_links() {
    use std::os::unix::fs::symlink;
    let w = Workspace::new();
    fs::write(w.0.join("ü-file"), b"abc").unwrap();
    fs::hard_link(w.0.join("ü-file"), w.0.join("alias")).unwrap();
    fs::create_dir(w.0.join("Directory")).unwrap();
    symlink(w.0.join("ü-file"), w.0.join("Link")).unwrap();
    let parent = anchored::root(&w.0).unwrap();
    for name in ["ü-file", "alias", "Directory"] {
        let actual = anchored::child_stamp(&parent, std::ffi::OsStr::new(name)).unwrap();
        assert_eq!(
            actual,
            Stamp::read(&fs::symlink_metadata(w.0.join(name)).unwrap())
        );
    }
    assert!(anchored::child_stamp(&parent, std::ffi::OsStr::new("Link")).is_err());
    assert!(anchored::directory(&parent, std::ffi::OsStr::new("alias")).is_err());
}

#[test]
fn preimport_unverified_folder_and_zip_exclude_reader_cleanup_before_normal_sqlite_open() {
    use std::io::Write;
    for zip in [false, true] {
        let w = Workspace::new();
        let (root, incoming) = incoming_fixture(&w);
        let library = Library::open(&root).unwrap();
        library
            .connection
            .execute_batch(
                "CREATE TABLE marker(value TEXT); INSERT INTO marker VALUES('original');",
            )
            .unwrap();
        drop(library);
        let source = if zip {
            let path = w.0.join("Owned.zip");
            let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
            writer
                .start_file("One.wav", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"abc").unwrap();
            writer.finish().unwrap();
            path
        } else {
            incoming.clone()
        };
        let control = ImportControl::default();
        let new_plan = || {
            if zip {
                prepare(&root, &source, true, &control, |_| {}).unwrap()
            } else {
                prepare_with_limits(
                    &root,
                    &source,
                    false,
                    &control,
                    |_| {},
                    Limits {
                        entries: 0,
                        ..LIMITS
                    },
                )
                .unwrap()
            }
        };
        let before = fs::read(root.join("catalog.sqlite3")).unwrap();
        let overlapping = super::super::storage::CatalogReader::open(&root).unwrap();
        let error =
            consume(&root, new_plan(), true, &control, &mut |_| {}, &mut |f| f()).unwrap_err();
        assert!(error.contains("Katalogprüfung läuft noch"));
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
        assert!(!root.join("packages").exists());
        drop(overlapping);
        let prepared = new_plan();
        if zip {
            authorize(&prepared, &root, &control).unwrap();
        }
        let mut checked = false;
        let result = consume(
            &root,
            prepared,
            true,
            &control,
            &mut |progress| {
                if progress.stage == "cataloging" && !checked {
                    checked = true;
                    assert!(super::super::storage::CatalogReader::open(&root).is_err());
                    assert_independent_writer_excluded(&root);
                }
            },
            &mut |f| f(),
        )
        .unwrap();
        assert!(checked);
        assert_eq!(
            fs::read(root.join(result.relative_path).join("One.wav")).unwrap(),
            b"abc"
        );
        assert_eq!(fs::read(incoming.join("One.wav")).unwrap(), b"abc");
    }
}

#[test]
fn preimport_low_descriptor_constructor_child() {
    let Ok(path) = std::env::var("PREIMPORT_OWNED_LOW_DESCRIPTOR_DIRECTORY") else {
        return;
    };
    let path = PathBuf::from(path);
    assert!(path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("preimport-owned-"));
    let directory = anchored::root(&path).unwrap();
    #[repr(C)]
    struct Limit {
        current: u64,
        maximum: u64,
    }
    unsafe extern "C" {
        fn getrlimit(resource: i32, limit: *mut Limit) -> i32;
        fn setrlimit(resource: i32, limit: *const Limit) -> i32;
    }
    let mut old = Limit {
        current: 0,
        maximum: 0,
    };
    // Darwin SDK RLIMIT_NOFILE; only this isolated test process is constrained.
    assert_eq!(unsafe { getrlimit(8, &mut old) }, 0);
    let limited = Limit {
        current: 32,
        maximum: old.maximum,
    };
    for mode in ["default", "controlled", "replacement"] {
        let name = format!(".import-owned-low-fd-{mode}");
        let control = ImportControl::default();
        assert_eq!(unsafe { setrlimit(8, &limited) }, 0);
        let mut held = Vec::new();
        while let Ok(file) = fs::File::open("/dev/null") {
            held.push(file);
        }
        let result = if mode == "default" {
            super::super::sprite_export::anchored::create_at(&directory, &name)
        } else {
            super::super::sprite_export::anchored::create_at_report_retained(
                &directory,
                &name,
                || {
                    control.mark_cleanup_failed();
                    control.cancel();
                    if mode == "replacement" {
                        // The failed open already captured EMFILE. One free descriptor lets the independent fixture actor write.
                        drop(held.pop());
                        fs::rename(path.join(&name), path.join("OriginalStage")).unwrap();
                        fs::create_dir(path.join(&name)).unwrap();
                        fs::write(path.join(&name).join("Unrelated.txt"), b"keep").unwrap();
                    }
                },
            )
        };
        drop(held);
        assert_eq!(unsafe { setrlimit(8, &old) }, 0);
        let error = result.unwrap_err();
        assert!(error.contains("os error 24"), "{error}");
        assert!(path.join(&name).is_dir());
        assert!(
            error.contains("nicht sicher entfernt"),
            "retained construction must be explicit: {error}"
        );
        assert!(
            error.contains(&name),
            "the caller must identify the potentially retained directory: {error}"
        );
        assert_eq!(control.cleanup_failed(), mode != "default");
        if mode == "replacement" {
            assert_eq!(
                fs::read(path.join(&name).join("Unrelated.txt")).unwrap(),
                b"keep"
            );
            assert!(path.join("OriginalStage").is_dir());
        } else {
            assert_eq!(fs::read_dir(path.join(&name)).unwrap().count(), 0);
        }
        println!("OWNED_LOW_DESCRIPTOR_RETAINED {mode}: {error}");
    }
}

#[test]
fn preimport_low_descriptor_constructor_has_explicit_retained_error() {
    let w = Workspace::new();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "library::preimport::tests::preimport_low_descriptor_constructor_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PREIMPORT_OWNED_LOW_DESCRIPTOR_DIRECTORY", &w.0)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(String::from_utf8_lossy(&child.stdout).contains("OWNED_LOW_DESCRIPTOR_RETAINED"));
}

#[test]
fn preimport_existing_directory_collision_does_not_report_owned_retention() {
    let w = Workspace::new();
    let name = ".import-unrelated-existing";
    fs::create_dir(w.0.join(name)).unwrap();
    fs::write(w.0.join(name).join("Unrelated.txt"), b"keep").unwrap();
    let directory = anchored::root(&w.0).unwrap();
    let control = ImportControl::default();
    let error =
        super::super::sprite_export::anchored::create_at_report_retained(&directory, name, || {
            control.mark_cleanup_failed()
        })
        .unwrap_err();
    assert!(!control.cleanup_failed());
    assert!(!error.contains("nicht sicher entfernt"));
    assert_eq!(
        fs::read(w.0.join(name).join("Unrelated.txt")).unwrap(),
        b"keep"
    );
}
