use super::*;
use crate::library::{ImportControl, Library};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "recovery-owned-{}-{}",
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
fn recovery_independent_writer_child() {
    let Ok(p) = std::env::var("RECOVERY_OWNED_WRITER_CATALOG") else {
        return;
    };
    let p = PathBuf::from(p);
    assert!(p
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("recovery-owned-"));
    let connection = rusqlite::Connection::open(p).unwrap();
    connection.busy_timeout(Duration::ZERO).unwrap();
    if std::env::var("RECOVERY_WAL_CRASH").as_deref() == Ok("1") {
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; UPDATE packages SET name='owned-wal-actor';").unwrap();
        std::process::exit(0);
    }
    let result = connection
        .execute_batch("BEGIN IMMEDIATE; UPDATE packages SET name='independent-writer'; COMMIT;");
    if std::env::var("RECOVERY_EXPECT_BUSY").as_deref() == Ok("1") {
        assert!(
            matches!(result,Err(rusqlite::Error::SqliteFailure(e,_)) if e.code==rusqlite::ErrorCode::DatabaseBusy),
            "{result:?}"
        );
        println!("OWNED_RECOVERY_WRITER_BUSY");
    } else {
        result.unwrap();
        println!("OWNED_RECOVERY_WRITER_COMMITTED");
    }
}
fn independent_writer(root: &std::path::Path, busy: bool) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "library::recovery::tests::recovery_independent_writer_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(
            "RECOVERY_OWNED_WRITER_CATALOG",
            root.join("catalog.sqlite3"),
        )
        .env("RECOVERY_EXPECT_BUSY", if busy { "1" } else { "0" })
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(if busy {
        "OWNED_RECOVERY_WRITER_BUSY"
    } else {
        "OWNED_RECOVERY_WRITER_COMMITTED"
    }));
}
#[test]
fn recovery_read_reservation_excludes_independent_writer_until_all_raw_guards_release() {
    let w = Workspace::new();
    let incoming = w.0.join("Incoming");
    fs::create_dir(&incoming).unwrap();
    fs::write(incoming.join("Original.wav"), b"owned-sound").unwrap();
    let mut library = Library::create(&w.0.join("Library ?%ä#")).unwrap();
    library.import_folder(&incoming).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let original = fs::read(root.join("catalog.sqlite3")).unwrap();
    let reservation = ReadReservation::open(
        &root,
        &ImportControl::default(),
        Instant::now(),
        Duration::from_secs(10),
    )
    .unwrap();
    reservation.validate().unwrap();
    independent_writer(&root, true);
    drop(reservation);
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), original);
    independent_writer(&root, false);
}

#[test]
fn recovery_cleanup_metadata_rejects_replacement_without_releasing_catalogue_lock() {
    use std::os::unix::fs::MetadataExt;
    let w = Workspace::new();
    let incoming = w.0.join("Incoming");
    fs::create_dir(&incoming).unwrap();
    fs::write(incoming.join("Original.wav"), b"owned-sound").unwrap();
    let mut library = Library::create(&w.0.join("Library")).unwrap();
    library.import_folder(&incoming).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let output = w.0.join("Output");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("Owned.wav"), b"owned-stage").unwrap();
    let m = fs::metadata(output.join("Owned.wav")).unwrap();
    let expected = (m.dev(), m.ino());
    let parent = anchored::root(&output).unwrap();
    let reservation = ReadReservation::open(
        &root,
        &ImportControl::default(),
        Instant::now(),
        Duration::from_secs(10),
    )
    .unwrap();
    fs::remove_file(output.join("Owned.wav")).unwrap();
    fs::hard_link(root.join("catalog.sqlite3"), output.join("Owned.wav")).unwrap();
    assert!(!crate::library::sprite_export::anchored::remove_owned(
        &parent,
        std::ffi::OsStr::new("Owned.wav"),
        expected,
        false
    ));
    independent_writer(&root, true);
    drop(reservation);
    assert!(output.join("Owned.wav").is_file());
}

#[test]
fn recovery_preparation_preserves_complete_owned_inventory_and_empty_directories() {
    let w = Workspace::new();
    let incoming = w.0.join("Incoming");
    fs::create_dir(&incoming).unwrap();
    fs::write(incoming.join("Original.wav"), b"owned-sound").unwrap();
    fs::write(incoming.join("LICENSE.txt"), b"owned-license").unwrap();
    let mut library = Library::create(&w.0.join("Library")).unwrap();
    library.import_folder(&incoming).unwrap();
    let root = library.root().to_owned();
    drop(library);
    fs::create_dir(root.join("Empty")).unwrap();
    fs::write(root.join(".support"), b"owned-hidden").unwrap();
    let destination = w.0.join("Backups");
    fs::create_dir(&destination).unwrap();
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let plan = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: destination.to_str().unwrap().into(),
        },
        &ImportControl::default(),
        "owned-prepare",
        |_| {},
    )
    .unwrap();
    assert!(plan
        .entries
        .iter()
        .any(|e| e.path == "Empty" && e.stamp.is_dir()));
    assert!(plan.entries.iter().any(|e| e.path.ends_with("LICENSE.txt")));
    assert!(plan.entries.iter().any(|e| e.path == ".support"));
    assert_eq!(plan.summary.external_reference_records, Some(0));
    assert!(!plan.destination.exists());
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
}
fn owned_library(w: &Workspace) -> PathBuf {
    let incoming = w.0.join("Incoming");
    fs::create_dir(&incoming).unwrap();
    fs::write(incoming.join("Original.wav"), b"owned-sound").unwrap();
    fs::write(incoming.join("LICENSE.txt"), b"owned-license").unwrap();
    fs::create_dir(incoming.join("EmptyCompanion")).unwrap();
    let mut library = Library::create(&w.0.join("Library")).unwrap();
    let package = library.import_folder(&incoming).unwrap();
    library.trash_package(package.id).unwrap();
    let external = w.0.join("External");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("Offline.wav"), b"external-never-copied").unwrap();
    library.reference_folder(&external).unwrap();
    fs::remove_dir_all(external).unwrap();
    let archive = w.0.join("owned.zip");
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("Nested/Other.wav", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"owned-zip-sound").unwrap();
        zip.start_file(
            "Nested/License.md",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"owned-zip-license").unwrap();
        zip.finish().unwrap();
    }
    library.import_zip(&archive).unwrap();
    library
        .connection
        .execute(
            "INSERT INTO asset_tags(asset_id,tag) SELECT id,'owned-tag' FROM assets LIMIT 1",
            [],
        )
        .unwrap();
    let folder = library.create_folder("Owned project", None).unwrap();
    let id: i64 = library
        .connection
        .query_row("SELECT id FROM active_assets LIMIT 1", [], |r| r.get(0))
        .unwrap();
    library.add_to_folder(folder.id, id).unwrap();
    let root = library.root().to_owned();
    drop(library);
    fs::create_dir(root.join("Empty")).unwrap();
    fs::write(root.join(".support"), b"owned-hidden").unwrap();
    fs::hard_link(root.join(".support"), root.join("ordinary-hardlink.txt")).unwrap();
    root
}
fn images(root: &std::path::Path) -> std::collections::BTreeMap<String, Option<Vec<u8>>> {
    fn walk(
        root: &std::path::Path,
        prefix: &str,
        out: &mut std::collections::BTreeMap<String, Option<Vec<u8>>>,
    ) {
        for e in fs::read_dir(root).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_str().unwrap().to_owned();
            let p = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if e.file_type().unwrap().is_dir() {
                out.insert(p.clone(), None);
                walk(&e.path(), &p, out)
            } else {
                out.insert(p, Some(fs::read(e.path()).unwrap()));
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, "", &mut out);
    out
}
fn direct_publish(
    op: &mut dyn FnMut() -> crate::library::Result<Option<String>>,
) -> crate::library::Result<Option<String>> {
    op()
}
#[test]
fn recovery_backup_streams_all_originals_metadata_trash_licenses_and_empty_dirs() {
    let w = Workspace::new();
    let root = owned_library(&w);
    let before = images(&root);
    let parent = w.0.join("Backups");
    fs::create_dir(&parent).unwrap();
    let control = ImportControl::default();
    let plan = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: parent.to_str().unwrap().into(),
        },
        &control,
        "owned-backup",
        |_| {},
    )
    .unwrap();
    let destination = plan.destination.clone();
    assert_eq!(plan.summary.external_reference_records, Some(1));
    let mut phases = Vec::new();
    let outcome = run_recovery(
        plan,
        &control,
        "owned-backup",
        |p| phases.push(p.phase),
        &mut direct_publish,
    );
    assert!(
        matches!(
            outcome,
            RecoveryOutcome::Published {
                durability: "synced",
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(images(&destination.join("library")), before);
    assert_eq!(images(&root), before);
    assert!(destination.join("manifest.json").is_file());
    assert!(phases.contains(&"verifyingCopy") && phases.contains(&"verifyingSource"));
    assert!(fs::read_dir(parent).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".game-assets-recovery-")));
}
#[test]
fn recovery_restore_fresh_root_needs_no_active_library_and_keeps_raw_catalogue_equal() {
    let w = Workspace::new();
    let root = owned_library(&w);
    let before = images(&root);
    let parent = w.0.join("Backups");
    fs::create_dir(&parent).unwrap();
    let control = ImportControl::default();
    let plan = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: parent.to_str().unwrap().into(),
        },
        &control,
        "owned-backup",
        |_| {},
    )
    .unwrap();
    let artifact = plan.destination.clone();
    let result = run_recovery(plan, &control, "owned-backup", |_| {}, &mut direct_publish);
    assert!(
        matches!(result, RecoveryOutcome::Published { .. }),
        "{result:?}"
    );
    let restored_parent = w.0.join("Restores");
    fs::create_dir(&restored_parent).unwrap();
    let plan = prepare_recovery(
        None,
        RecoveryRequest::Restore {
            artifact_root: artifact.to_str().unwrap().into(),
            destination_parent: restored_parent.to_str().unwrap().into(),
        },
        &control,
        "owned-restore",
        |_| {},
    )
    .unwrap();
    let destination = plan.destination.clone();
    let result = run_recovery(plan, &control, "owned-restore", |_| {}, &mut direct_publish);
    assert!(
        matches!(
            result,
            RecoveryOutcome::Published {
                durability: "synced",
                ..
            }
        ),
        "{result:?}"
    );
    assert_eq!(images(&destination), before);
    assert_eq!(images(&root), before);
    let restored = Library::open(&destination).unwrap();
    assert_eq!(restored.trash_entries().unwrap().len(), 1);
    assert!(restored
        .folders()
        .unwrap()
        .iter()
        .any(|f| f.name == "Owned project"));
}
fn backup_plan(w: &Workspace) -> (PathBuf, PreparedRecovery, ImportControl) {
    let root = owned_library(w);
    let parent = w.0.join("Backups");
    fs::create_dir(&parent).unwrap();
    let control = ImportControl::default();
    let plan = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: parent.to_str().unwrap().into(),
        },
        &control,
        "owned-failure",
        |_| {},
    )
    .unwrap();
    (root, plan, control)
}
#[test]
fn recovery_run_rejects_changed_prepared_file_parent_topology_and_destination() {
    for mutation in ["file", "parent", "added", "destination", "mtimeReset"] {
        let w = Workspace::new();
        let (root, plan, control) = backup_plan(&w);
        let destination = plan.destination.clone();
        let entry = plan
            .entries
            .iter()
            .find(|e| e.path.ends_with("Original.wav"))
            .unwrap()
            .path
            .clone();
        let file = root.join(entry);
        match mutation {
            "file" => {
                fs::rename(&file, file.with_extension("old")).unwrap();
                fs::write(&file, b"other-sound").unwrap();
            }
            "parent" => {
                let parent = file.parent().unwrap();
                fs::rename(parent, parent.with_extension("old")).unwrap();
                fs::create_dir(parent).unwrap();
            }
            "added" => fs::write(root.join("added.txt"), b"added").unwrap(),
            "destination" => fs::create_dir(&destination).unwrap(),
            "mtimeReset" => {
                let old = fs::metadata(&file).unwrap().modified().unwrap();
                fs::write(&file, b"other-sound").unwrap();
                fs::File::options()
                    .write(true)
                    .open(&file)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(old))
                    .unwrap();
            }
            _ => unreachable!(),
        };
        let actor_image = images(&root);
        let result = run_recovery(plan, &control, "owned-failure", |_| {}, &mut direct_publish);
        assert!(
            matches!(result, RecoveryOutcome::Failed { .. }),
            "{mutation}: {result:?}"
        );
        assert_eq!(images(&root), actor_image);
        if mutation != "destination" {
            assert!(!destination.exists());
        }
        assert!(fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".game-assets-recovery-")));
    }
}
#[test]
fn recovery_stop_before_copy_and_during_stream_keeps_sources_and_cleans_owned_stage() {
    for phase in ["before", "copying", "verifyingCopy", "verifyingSource"] {
        let w = Workspace::new();
        let (root, plan, control) = backup_plan(&w);
        let before = images(&root);
        let destination = plan.destination.clone();
        if phase == "before" {
            control.cancel()
        }
        let result = run_recovery(
            plan,
            &control,
            "owned-failure",
            |p| {
                if p.phase == phase {
                    control.cancel()
                }
            },
            &mut direct_publish,
        );
        assert!(
            matches!(
                result,
                RecoveryOutcome::Cancelled {
                    cleanup: "complete",
                    ..
                }
            ),
            "{phase}: {result:?}"
        );
        assert_eq!(images(&root), before);
        assert!(!destination.exists());
        assert_eq!(
            fs::read_dir(destination.parent().unwrap()).unwrap().count(),
            0
        );
    }
}
#[test]
fn recovery_committed_publish_survives_late_stop_and_publisher_error() {
    let w = Workspace::new();
    let (root, plan, control) = backup_plan(&w);
    let before = images(&root);
    let destination = plan.destination.clone();
    let mut publisher = |op: &mut dyn FnMut() -> crate::library::Result<Option<String>>| {
        op()?;
        control.cancel();
        Err("owned post-commit acknowledgement failed".into())
    };
    let result = run_recovery(plan, &control, "owned-failure", |_| {}, &mut publisher);
    assert!(
        matches!(
            result,
            RecoveryOutcome::Published {
                durability: "uncertain",
                ..
            }
        ),
        "{result:?}"
    );
    assert_eq!(images(&root), before);
    assert_eq!(images(&destination.join("library")), before);
}
#[test]
fn recovery_reduced_inventory_limits_and_sparse_large_file_plan_are_read_only() {
    let w = Workspace::new();
    let root = owned_library(&w);
    let extra = root.join("owned-large.bin");
    let f = fs::File::create(&extra).unwrap();
    f.set_len(4 * 1024_u64.pow(3) + 1).unwrap();
    drop(f);
    let parent = w.0.join("Backups");
    fs::create_dir(&parent).unwrap();
    let control = ImportControl::default();
    let request = || RecoveryRequest::Backup {
        destination_parent: parent.to_str().unwrap().into(),
    };
    let plan = prepare_recovery(Some(&root), request(), &control, "owned-large", |_| {}).unwrap();
    assert!(plan.summary.logical_bytes > 4 * 1024_u64.pow(3));
    assert!(!plan.destination.exists());
    drop(plan);
    for which in ["entries", "bytes", "memory", "time"] {
        let mut limits = RecoveryLimits::default();
        match which {
            "entries" => limits.max_entries = 1,
            "bytes" => limits.max_logical_bytes = 1024,
            "memory" => limits.max_retained_bytes = 1024,
            "time" => limits.preparation_deadline_ms = 0,
            _ => unreachable!(),
        };
        let result = prepare_with_limits(
            Some(&root),
            request(),
            &control,
            "owned-limit",
            |_| {},
            limits,
        );
        assert!(result.is_err(), "{which}");
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
    }
    assert_eq!(fs::metadata(&extra).unwrap().len(), 4 * 1024_u64.pow(3) + 1);
}
#[test]
fn recovery_read_reservation_refuses_overlapping_reader_and_known_sql_aliases() {
    let w = Workspace::new();
    let root = owned_library(&w);
    let control = ImportControl::default();
    let reader = CatalogReader::open(&root).unwrap();
    assert!(
        ReadReservation::open(&root, &control, Instant::now(), Duration::from_secs(10)).is_err()
    );
    drop(reader);
    let reservation =
        ReadReservation::open(&root, &control, Instant::now(), Duration::from_secs(10)).unwrap();
    independent_writer(&root, true);
    drop(reservation);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    fs::hard_link(
        root.join("catalog.sqlite3"),
        root.join("unindexed-catalog-alias"),
    )
    .unwrap();
    assert!(
        ReadReservation::open(&root, &control, Instant::now(), Duration::from_secs(10)).is_err()
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
}
#[test]
fn recovery_has_moved_and_named_parent_replacement_fail_closed() {
    let w = Workspace::new();
    let root = owned_library(&w);
    let control = ImportControl::default();
    let reservation =
        ReadReservation::open(&root, &control, Instant::now(), Duration::from_secs(10)).unwrap();
    fs::rename(root.join("catalog.sqlite3"), root.join("detached-original")).unwrap();
    let mut moved = -1_i32;
    let rc = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            reservation.connection.as_ref().unwrap().handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            std::ptr::from_mut(&mut moved).cast(),
        )
    };
    assert_eq!((rc, moved), (0, 1));
    assert!(reservation.validate().is_err());
    drop(reservation);
}
#[test]
fn recovery_normal_named_uri_wal_transition_cannot_create_source_shm() {
    let w = Workspace::new();
    let root = owned_library(&w);
    let reader = CatalogReader::open_reserved(&root).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "library::recovery::tests::recovery_independent_writer_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(
            "RECOVERY_OWNED_WRITER_CATALOG",
            root.join("catalog.sqlite3"),
        )
        .env("RECOVERY_WAL_CRASH", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    // The owned child exits without SQLite cleanup, leaving genuine committed WAL.
    fs::remove_file(root.join("catalog.sqlite3-shm")).unwrap();
    let catalog = fs::read(root.join("catalog.sqlite3")).unwrap();
    let wal = fs::read(root.join("catalog.sqlite3-wal")).unwrap();
    let read = normal_read_only(&root.join("catalog.sqlite3")).unwrap();
    read.pragma_update(None, "query_only", true).unwrap();
    read.execute_batch("BEGIN DEFERRED").unwrap();
    let r = read.query_row("SELECT name FROM packages LIMIT 1", [], |r| {
        r.get::<_, String>(0)
    });
    assert!(r.is_err());
    drop(read);
    assert!(!root.join("catalog.sqlite3-shm").exists());
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), catalog);
    assert_eq!(fs::read(root.join("catalog.sqlite3-wal")).unwrap(), wal);
    assert!(reader.validate().is_err());
    drop(reader);
}
#[test]
fn recovery_failed_cleanup_reports_retained_unknown_stage_and_preserves_originals() {
    let w = Workspace::new();
    let (root, plan, control) = backup_plan(&w);
    let before = images(&root);
    let parent = plan.destination.parent().unwrap().to_owned();
    let final_path = plan.destination.clone();
    let mut injected = false;
    let result = run_recovery(
        plan,
        &control,
        "owned-residue",
        |p| {
            if p.phase == "copying" && !injected {
                injected = true;
                let stage = fs::read_dir(&parent)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".game-assets-recovery-")
                    })
                    .unwrap();
                fs::write(stage.join("unknown.txt"), b"preserve-unknown").unwrap();
                control.cancel();
            }
        },
        &mut direct_publish,
    );
    assert!(
        matches!(
            result,
            RecoveryOutcome::Failed {
                cleanup: "retained",
                reason: "cleanupIncomplete",
                residue_path: Some(_),
                ..
            }
        ),
        "{result:?}"
    );
    assert_eq!(images(&root), before);
    assert!(!final_path.exists());
    let stage = fs::read_dir(parent)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::read(stage.join("unknown.txt")).unwrap(),
        b"preserve-unknown"
    );
}
#[test]
fn recovery_restore_refuses_changed_extra_or_unsafe_artifacts_without_writes() {
    for mutation in ["manifest", "extra", "missing", "alias"] {
        let w = Workspace::new();
        let (root, plan, control) = backup_plan(&w);
        let artifact = plan.destination.clone();
        let result = run_recovery(plan, &control, "owned-backup", |_| {}, &mut direct_publish);
        assert!(
            matches!(result, RecoveryOutcome::Published { .. }),
            "{result:?}"
        );
        let parent = w.0.join("Restores");
        fs::create_dir(&parent).unwrap();
        match mutation {
            "manifest" => fs::write(artifact.join("manifest.json"), b"{\"version\":99}").unwrap(),
            "extra" => fs::write(artifact.join("extra.txt"), b"extra").unwrap(),
            "missing" => {
                let original = images(&artifact.join("library"))
                    .keys()
                    .find(|p| p.ends_with("Original.wav"))
                    .unwrap()
                    .clone();
                fs::remove_file(artifact.join("library").join(original)).unwrap();
            }
            "alias" => {
                fs::remove_file(artifact.join("manifest.json")).unwrap();
                fs::hard_link(
                    artifact.join("library/catalog.sqlite3"),
                    artifact.join("manifest.json"),
                )
                .unwrap();
            }
            _ => unreachable!(),
        };
        let source_image = images(&artifact);
        let live_image = images(&root);
        let result = prepare_recovery(
            Some(&root),
            RecoveryRequest::Restore {
                artifact_root: artifact.to_str().unwrap().into(),
                destination_parent: parent.to_str().unwrap().into(),
            },
            &control,
            "owned-restore",
            |_| {},
        );
        assert!(result.is_err(), "{mutation}");
        assert_eq!(images(&artifact), source_image);
        assert_eq!(images(&root), live_image);
        assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
    }
}
#[test]
fn recovery_run_time_budget_and_source_hash_mismatch_never_publish() {
    for mutation in ["time", "hash"] {
        let w = Workspace::new();
        let (root, mut plan, control) = backup_plan(&w);
        let before = images(&root);
        let final_path = plan.destination.clone();
        if mutation == "time" {
            plan.limits.run_deadline_ms = 0;
        } else {
            plan.entries
                .iter_mut()
                .find(|e| e.path.ends_with("Original.wav"))
                .unwrap()
                .sha256 = Some("0".repeat(64));
        }
        let result = run_recovery(plan, &control, "owned-budget", |_| {}, &mut direct_publish);
        assert!(
            matches!(result, RecoveryOutcome::Failed { .. }),
            "{mutation}: {result:?}"
        );
        assert!(!final_path.exists());
        assert_eq!(images(&root), before);
        assert_eq!(
            fs::read_dir(final_path.parent().unwrap()).unwrap().count(),
            0
        );
    }
}
#[test]
fn recovery_prepare_rejects_symlink_special_staging_and_live_target() {
    use std::os::unix::fs::symlink;
    for mutation in ["symlink", "staging", "liveTarget"] {
        let w = Workspace::new();
        let (root, plan, control) = backup_plan(&w);
        let mut parent = plan.destination.parent().unwrap().to_owned();
        drop(plan);
        match mutation {
            "symlink" => symlink(w.0.join("outside"), root.join("link.txt")).unwrap(),
            "staging" => fs::create_dir(root.join(".import-unfinished")).unwrap(),
            "liveTarget" => parent = root.clone(),
            _ => unreachable!(),
        };
        let result = prepare_recovery(
            Some(&root),
            RecoveryRequest::Backup {
                destination_parent: parent.to_str().unwrap().into(),
            },
            &control,
            "owned-topology",
            |_| {},
        );
        assert!(result.is_err(), "{mutation}");
    }
}

#[test]
fn recovery_serialized_rust_contract_uses_actual_producers_and_outcomes() {
    let mut examples = serde_json::Map::new();
    let w = Workspace::new();
    let (root, plan, control) = backup_plan(&w);
    let before = images(&root);
    let destination = plan.destination.clone();
    examples.insert(
        "ready".into(),
        serde_json::to_value(plan.reply("owned-failure")).unwrap(),
    );
    let mut progress = Vec::new();
    let outcome = run_recovery(
        plan,
        &control,
        "owned-failure",
        |p| {
            assert!(progress.len() < 128);
            progress.push(serde_json::to_value(p).unwrap())
        },
        &mut direct_publish,
    );
    assert!(matches!(
        outcome,
        RecoveryOutcome::Published {
            durability: "synced",
            ..
        }
    ));
    examples.insert(
        "publishedSynced".into(),
        serde_json::to_value(outcome).unwrap(),
    );
    examples.insert("progress".into(), progress.into());
    assert_eq!(images(&root), before);
    assert_eq!(images(&destination.join("library")), before);
    let payload_proof: Vec<_> = before.iter().map(|(path,bytes)| {let sha256=bytes.as_ref().map(|bytes| {let mut sha=crate::library::duplicates::sdk::Sha256::new();for chunk in bytes.chunks(65536) {sha.update(chunk);}sha.finish().iter().map(|b|format!("{b:02x}")).collect::<String>()});serde_json::json!({"relativePath":path,"directory":bytes.is_none(),"bytes":bytes.as_ref().map(Vec::len),"sha256":sha256})}).collect();
    if let Ok(path) = std::env::var("RECOVERY_OWNED_PAYLOAD_FIXTURE") {
        let path = PathBuf::from(path);
        assert!(path.starts_with("/private/tmp") && !path.exists());
        fs::write(path,serde_json::to_vec_pretty(&serde_json::json!({"source":"actual owned folder+ZIP/trash/tag/project/offline-external fixture","copiedSourceAndOriginalImagesEqual":true,"entries":payload_proof})).unwrap()).unwrap();
    }
    let w = Workspace::new();
    let (_, plan, control) = backup_plan(&w);
    let mut late = |op: &mut dyn FnMut() -> crate::library::Result<Option<String>>| {
        op()?;
        control.cancel();
        Err("owned late acknowledgement failure".into())
    };
    let outcome = run_recovery(plan, &control, "owned-failure", |_| {}, &mut late);
    assert!(matches!(
        outcome,
        RecoveryOutcome::Published {
            durability: "uncertain",
            ..
        }
    ));
    examples.insert(
        "publishedUncertain".into(),
        serde_json::to_value(outcome).unwrap(),
    );
    let w = Workspace::new();
    let (root, plan, control) = backup_plan(&w);
    let parent = plan.destination.parent().unwrap().to_owned();
    drop(plan);
    let reader = CatalogReader::open(&root).unwrap();
    let error = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: parent.to_str().unwrap().into(),
        },
        &control,
        "owned-busy",
        |_| {},
    )
    .err()
    .unwrap();
    assert_eq!(error.reason, "busy");
    examples.insert(
        "unavailable".into(),
        serde_json::to_value(error.preparation("owned-busy", RecoveryKind::Backup, &control))
            .unwrap(),
    );
    drop(reader);
    control.cancel();
    let error = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: parent.to_str().unwrap().into(),
        },
        &control,
        "owned-stop",
        |_| {},
    )
    .err()
    .unwrap();
    examples.insert(
        "preparationCancelled".into(),
        serde_json::to_value(error.preparation("owned-stop", RecoveryKind::Backup, &control))
            .unwrap(),
    );
    let w = Workspace::new();
    let (_, plan, control) = backup_plan(&w);
    control.cancel();
    let outcome = run_recovery(plan, &control, "owned-failure", |_| {}, &mut direct_publish);
    assert!(matches!(
        outcome,
        RecoveryOutcome::Cancelled {
            cleanup: "complete",
            ..
        }
    ));
    examples.insert(
        "runCancelled".into(),
        serde_json::to_value(outcome).unwrap(),
    );
    let w = Workspace::new();
    let (_, plan, control) = backup_plan(&w);
    let parent = plan.destination.parent().unwrap().to_owned();
    let mut injected = false;
    let outcome = run_recovery(
        plan,
        &control,
        "owned-failure",
        |p| {
            if p.phase == "copying" && !injected {
                injected = true;
                let stage = fs::read_dir(&parent)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".game-assets-recovery-")
                    })
                    .unwrap();
                fs::write(stage.join("unknown.txt"), b"owned-preserved").unwrap();
                control.cancel()
            }
        },
        &mut direct_publish,
    );
    assert!(matches!(
        outcome,
        RecoveryOutcome::Failed {
            cleanup: "retained",
            reason: "cleanupIncomplete",
            ..
        }
    ));
    examples.insert(
        "failedRetained".into(),
        serde_json::to_value(outcome).unwrap(),
    );
    if let Ok(path) = std::env::var("RECOVERY_OWNED_DTO_FIXTURE") {
        let path = PathBuf::from(path);
        assert!(path.starts_with("/private/tmp") && !path.exists());
        fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::Value::Object(examples)).unwrap(),
        )
        .unwrap();
    }
}
#[test]
fn recovery_final_publisher_rechecks_source_versions_after_sync_and_progress() {
    let w = Workspace::new();
    let (root, plan, control) = backup_plan(&w);
    let file = root.join(
        plan.entries
            .iter()
            .find(|e| e.path.ends_with("Original.wav"))
            .unwrap()
            .path
            .clone(),
    );
    let destination = plan.destination.clone();
    let mut actor_image = images(&root);
    let key = file
        .strip_prefix(&root)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let result = run_recovery(
        plan,
        &control,
        "owned-last-check",
        |p| {
            if p.phase == "publishing" {
                let old = fs::metadata(&file).unwrap().modified().unwrap();
                fs::write(&file, b"other-sound").unwrap();
                fs::File::options()
                    .write(true)
                    .open(&file)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(old))
                    .unwrap();
                actor_image.insert(key.clone(), Some(b"other-sound".to_vec()));
            }
        },
        &mut direct_publish,
    );
    assert!(
        matches!(
            result,
            RecoveryOutcome::Failed {
                reason: "sourceChanged",
                ..
            }
        ),
        "{result:?}"
    );
    assert!(!destination.exists());
    assert_eq!(images(&root), actor_image);
    assert_eq!(
        fs::read_dir(destination.parent().unwrap()).unwrap().count(),
        0
    );
}

#[test]
fn recovery_schema_refuses_truncated_audio_metadata_without_changing_originals() {
    let w = Workspace::new();
    let root = owned_library(&w);
    let connection = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TABLE audio_metadata; CREATE TABLE audio_metadata(file_id INTEGER PRIMARY KEY);",
        )
        .unwrap();
    assert_eq!(
        connection
            .pragma_query_value::<String, _>(None, "integrity_check", |r| r.get(0))
            .unwrap(),
        "ok"
    );
    drop(connection);
    let library = Library::open_read_only(&root).unwrap();
    assert!(library.file_audio(1).is_err());
    drop(library);
    let before = images(&root);
    let destination = w.0.join("Backups");
    fs::create_dir(&destination).unwrap();
    let result = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: destination.to_str().unwrap().into(),
        },
        &ImportControl::default(),
        "owned-schema-audio",
        |_| {},
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("incompatible audio table admitted"),
    };
    assert_eq!(error.reason, "unsupported", "{}", error.message);
    assert_eq!(images(&root), before);
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
}

fn assert_owned_schema_refused(sql: &str) {
    let w = Workspace::new();
    let root = owned_library(&w);
    let connection = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
    connection.execute_batch(sql).unwrap();
    drop(connection);
    let before = images(&root);
    let destination = w.0.join("Backups");
    fs::create_dir(&destination).unwrap();
    let result = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: destination.to_str().unwrap().into(),
        },
        &ImportControl::default(),
        "owned-schema-layout",
        |_| {},
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("incompatible schema admitted: {sql}"),
    };
    assert_eq!(error.reason, "unsupported", "{sql}: {}", error.message);
    assert_eq!(images(&root), before, "{sql}");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0, "{sql}");
}

#[test]
fn recovery_schema_refuses_every_required_table_and_view_column_change() {
    let w = Workspace::new();
    let root = owned_library(&w);
    let connection = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
    let view_sql: Vec<String> = connection
        .prepare("SELECT sql FROM sqlite_schema WHERE type='view' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(view_sql.len(), 3);
    let mut changed_columns = 0;
    for (object, view_base) in [
        ("packages", None),
        ("assets", None),
        ("asset_files", None),
        ("external_sources", None),
        ("external_files", None),
        ("asset_tags", None),
        ("package_tags", None),
        ("audio_metadata", None),
        ("trash", None),
        ("folders", None),
        ("folder_assets", None),
        ("active_packages", Some("packages")),
        ("active_assets", Some("assets")),
        ("active_asset_files", Some("asset_files")),
    ] {
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_xinfo(?1) ORDER BY cid LIMIT 8")
            .unwrap()
            .query_map([object], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(!columns.is_empty() && columns.len() < 8);
        for column in &columns {
            let sql = if let Some(base) = view_base {
                let names = columns
                    .iter()
                    .map(|name| {
                        if name == column {
                            "unsupported_column"
                        } else {
                            name.as_str()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "DROP VIEW {object}; CREATE VIEW {object} ({names}) AS SELECT * FROM {base};"
                )
            } else {
                format!("DROP VIEW active_assets; DROP VIEW active_asset_files; DROP VIEW active_packages; ALTER TABLE {object} RENAME COLUMN {column} TO unsupported_column; {};", view_sql.join(";"))
            };
            assert_owned_schema_refused(&sql);
            changed_columns += 1;
        }
    }
    assert_eq!(changed_columns, 53);
}

#[test]
fn recovery_schema_refuses_incompatible_column_shapes_and_rebound_restore_artifact() {
    for sql in [
        "ALTER TABLE audio_metadata ADD COLUMN unsupported_column TEXT;",
        "ALTER TABLE audio_metadata ADD COLUMN unsupported_column INTEGER GENERATED ALWAYS AS (file_id) VIRTUAL;",
        "DROP TABLE audio_metadata; CREATE TABLE audio_metadata(file_id INTEGER PRIMARY KEY,duration_seconds REAL,sample_rate INTEGER,channels TEXT,codec TEXT,error TEXT);",
        "DROP TABLE audio_metadata; CREATE TABLE audio_metadata(file_id INTEGER PRIMARY KEY,duration_seconds REAL,sample_rate INTEGER,channels INTEGER GENERATED ALWAYS AS (sample_rate) VIRTUAL,codec TEXT,error TEXT);",
        "DROP TABLE audio_metadata; CREATE TABLE audio_metadata(file_id INTEGER PRIMARY KEY,duration_seconds REAL,sample_rate INTEGER,channels INTEGER,codec TEXT NOT NULL,error TEXT);",
        "DROP TABLE audio_metadata; CREATE TABLE audio_metadata(file_id INTEGER,duration_seconds REAL,sample_rate INTEGER,channels INTEGER,codec TEXT,error TEXT);",
        "DROP TABLE audio_metadata; CREATE TABLE audio_metadata(file_id INTEGER PRIMARY KEY,duration_seconds REAL,sample_rate INTEGER,channels INTEGER,error TEXT,codec TEXT);",
        "DROP TABLE trash; CREATE TABLE trash(id INTEGER PRIMARY KEY,package_id INTEGER UNIQUE REFERENCES packages(id),file_id INTEGER UNIQUE REFERENCES asset_files(id),deleted_at INTEGER NOT NULL DEFAULT 0);",
    ] { assert_owned_schema_refused(sql); }
    // A malformed artifact schema is refused after its real manifest/source bytes were rebound.
    let w = Workspace::new();
    let (_, plan, control) = backup_plan(&w);
    let artifact = plan.destination.clone();
    assert!(matches!(
        run_recovery(
            plan,
            &control,
            "owned-schema-backup",
            |_| {},
            &mut direct_publish
        ),
        RecoveryOutcome::Published { .. }
    ));
    let connection = rusqlite::Connection::open(artifact.join("library/catalog.sqlite3")).unwrap();
    connection
        .execute_batch(
            "DROP TABLE audio_metadata; CREATE TABLE audio_metadata(file_id INTEGER PRIMARY KEY);",
        )
        .unwrap();
    drop(connection);
    let manifest_path = artifact.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let bytes = fs::read(artifact.join("library/catalog.sqlite3")).unwrap();
    let entry = manifest["entries"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["path"] == "catalog.sqlite3")
        .unwrap();
    let mut sha = crate::library::duplicates::sdk::Sha256::new();
    for chunk in bytes.chunks(65536) {
        sha.update(chunk);
    }
    entry["sha256"] =
        serde_json::Value::String(sha.finish().iter().map(|b| format!("{b:02x}")).collect());
    entry["bytes"] = serde_json::Value::from(bytes.len() as u64);
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let before = images(&artifact);
    let restore_parent = w.0.join("Restores");
    fs::create_dir(&restore_parent).unwrap();
    let result = prepare_recovery(
        None,
        RecoveryRequest::Restore {
            artifact_root: artifact.to_str().unwrap().into(),
            destination_parent: restore_parent.to_str().unwrap().into(),
        },
        &ImportControl::default(),
        "owned-schema-restore",
        |_| {},
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("incompatible restore artifact admitted"),
    };
    assert_eq!(error.reason, "unsupported", "{}", error.message);
    assert_eq!(images(&artifact), before);
    assert_eq!(fs::read_dir(restore_parent).unwrap().count(), 0);
}
