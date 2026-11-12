use super::*;
use std::fs;

struct Fixture {
    workspace: PathBuf,
    root: PathBuf,
    source: PathBuf,
    managed_file: PathBuf,
    session: Session,
    entry: TrashRemovalEntry,
    other: i64,
}
impl Fixture {
    fn new() -> Self {
        let workspace = std::env::temp_dir().join(format!(
            "trash-removal-owned-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let source = workspace.join("External");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("One.wav"), b"original sound").unwrap();
        fs::write(source.join("Two.ogg"), b"original variant").unwrap();
        let managed_source = workspace.join("Managed");
        fs::create_dir(&managed_source).unwrap();
        fs::write(managed_source.join("Kept.wav"), b"managed sound").unwrap();
        let mut library = Library::create(&workspace.join("Library")).unwrap();
        let managed = library.import_folder(&managed_source).unwrap();
        let root = library.root().to_owned();
        let c = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
        let path: String = c.query_row("SELECT f.relative_path FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE a.package_id=?1", [managed.id], |r| r.get(0)).unwrap();
        let managed_file = root.join(path);
        let package = library.reference_folder(&source).unwrap();
        let other = library.reference_folder(&source).unwrap().id;
        let file: i64 = c.query_row("SELECT f.id FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE a.package_id=?1 ORDER BY f.id LIMIT 1", [package.id], |r| r.get(0)).unwrap();
        library.trash_file(file).unwrap();
        library.trash_package(package.id).unwrap();
        c.execute(
            "INSERT INTO asset_tags SELECT id,'keep until removal' FROM assets WHERE package_id=?1",
            [package.id],
        )
        .unwrap();
        c.execute(
            "INSERT INTO package_tags VALUES(?1,'removed tag')",
            [package.id],
        )
        .unwrap();
        c.execute("INSERT INTO audio_metadata(file_id,codec) SELECT f.id,'fixture' FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE a.package_id=?1", [package.id]).unwrap();
        c.execute("INSERT INTO folders(name) VALUES('Project')", [])
            .unwrap();
        c.execute("INSERT INTO folder_assets SELECT (SELECT id FROM folders LIMIT 1),id FROM assets WHERE package_id=?1", [package.id]).unwrap();
        let row = library
            .trash_entries()
            .unwrap()
            .into_iter()
            .find(|e| e.package_id == package.id && e.file_id.is_none())
            .unwrap();
        let entry = TrashRemovalEntry {
            id: row.id,
            package_id: package.id,
            name: row.name,
            deleted_at: row.deleted_at,
        };
        drop(c);
        drop(library);
        let session = Session::default();
        session.set_root(root.clone()).unwrap();
        Self {
            workspace,
            root,
            source,
            managed_file,
            session,
            entry,
            other,
        }
    }
    fn catalog(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.root.join("catalog.sqlite3")).unwrap()
    }
    fn bytes(&self) -> Vec<Vec<u8>> {
        [
            self.source.join("One.wav"),
            self.source.join("Two.ogg"),
            self.managed_file.clone(),
        ]
        .into_iter()
        .map(|p| fs::read(p).unwrap())
        .collect()
    }
    fn hashes(&self) -> String {
        let result = std::process::Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .args([
                self.source.join("One.wav"),
                self.source.join("Two.ogg"),
                self.managed_file.clone(),
            ])
            .output()
            .unwrap();
        assert!(result.status.success());
        String::from_utf8(result.stdout).unwrap()
    }
}
static NEXT: AtomicU64 = AtomicU64::new(0);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.workspace);
    }
}

#[test]
fn trash_removal_cleans_whole_external_catalog_and_preserves_originals_and_other_references() {
    let f = Fixture::new();
    let before = f.bytes();
    let hashes = f.hashes();
    let prepared = f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .unwrap();
    assert_eq!(prepared.package_name, "External");
    assert_eq!(prepared.external_root, f.source.to_str().unwrap());
    assert_eq!((prepared.asset_count, prepared.file_count), (2, 2));
    f.session.remove_prepared_trash(&prepared.token).unwrap();
    let c = f.catalog();
    for table in [
        "asset_tags",
        "package_tags",
        "audio_metadata",
        "folder_assets",
        "trash",
    ] {
        assert_eq!(
            c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    assert_eq!(
        c.query_row("SELECT count(*) FROM packages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM external_files", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM external_sources WHERE package_id=?1",
            [f.other],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(f.bytes(), before);
    assert_eq!(f.hashes(), hashes);
}

#[test]
fn trash_removal_rejects_managed_file_restored_and_stale_clicked_entries() {
    let f = Fixture::new();
    for entry in [
        TrashRemovalEntry {
            id: f.entry.id + 500,
            ..f.entry.clone()
        },
        TrashRemovalEntry {
            package_id: f.other,
            ..f.entry.clone()
        },
        TrashRemovalEntry {
            name: "stale name".into(),
            ..f.entry.clone()
        },
        TrashRemovalEntry {
            deleted_at: f.entry.deleted_at + 1,
            ..f.entry.clone()
        },
    ] {
        assert!(f.session.prepare_trash_removal(&f.root, entry).is_err());
    }
    let mut library = Library::open(&f.root).unwrap();
    let file_row = library
        .trash_entries()
        .unwrap()
        .into_iter()
        .find(|e| e.file_id.is_some())
        .unwrap();
    assert!(f
        .session
        .prepare_trash_removal(
            &f.root,
            TrashRemovalEntry {
                id: file_row.id,
                package_id: file_row.package_id,
                name: file_row.name,
                deleted_at: file_row.deleted_at
            }
        )
        .is_err());
    let c = f.catalog();
    let managed: i64 = c
        .query_row("SELECT id FROM packages WHERE name='Managed'", [], |r| {
            r.get(0)
        })
        .unwrap();
    library.trash_package(managed).unwrap();
    let row = library
        .trash_entries()
        .unwrap()
        .into_iter()
        .find(|e| e.package_id == managed)
        .unwrap();
    assert!(f
        .session
        .prepare_trash_removal(
            &f.root,
            TrashRemovalEntry {
                id: row.id,
                package_id: managed,
                name: row.name,
                deleted_at: row.deleted_at
            }
        )
        .is_err());
    library.restore_trash(f.entry.id).unwrap();
    assert!(f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .is_err());
}

#[test]
fn trash_removal_requires_complete_external_descendant_mappings() {
    let f = Fixture::new();
    let c = f.catalog();
    c.execute("DELETE FROM external_files WHERE file_id IN (SELECT f.id FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE a.package_id=?1 LIMIT 1)", [f.entry.package_id]).unwrap();
    assert!(f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .is_err());
}

#[test]
fn trash_removal_rechecks_displayed_fields_unique_path_identity_counts_and_trigger_policy() {
    for sql in [
        "UPDATE packages SET name='changed' WHERE id=?1",
        "UPDATE packages SET source_path='changed provenance' WHERE id=?1",
        "UPDATE packages SET relative_path='external/replaced-package' WHERE id=?1",
        "UPDATE external_sources SET root='/changed/recorded/root' WHERE package_id=?1",
        "UPDATE external_sources SET identity='changed identity' WHERE package_id=?1",
        "UPDATE trash SET deleted_at=deleted_at+1 WHERE package_id=?1",
        "DELETE FROM trash WHERE package_id=?1",
        "INSERT INTO assets(package_id,name,original_folder) VALUES(?1,'new descendant','')",
        "DELETE FROM external_files WHERE file_id IN (SELECT f.id FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE a.package_id=?1 LIMIT 1)",
    ] {
        let f = Fixture::new();
        let before = f.hashes();
        let reply = f.session.prepare_trash_removal(&f.root, f.entry.clone()).unwrap();
        let c = f.catalog();
        c.execute(sql, [f.entry.package_id]).unwrap();
        assert!(f.session.remove_prepared_trash(&reply.token).is_err(), "{sql}");
        assert!(f.session.remove_prepared_trash(&reply.token).is_err(), "failed run still consumes token");
        assert_eq!(c.query_row("SELECT count(*) FROM packages WHERE id=?1", [f.entry.package_id], |r| r.get::<_,i64>(0)).unwrap(), 1);
        assert_eq!(f.hashes(), before);
    }
    let f = Fixture::new();
    let reply = f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .unwrap();
    f.catalog().execute_batch("CREATE TRIGGER forbidden_cleanup AFTER DELETE ON external_files BEGIN DELETE FROM package_tags; END").unwrap();
    assert!(f
        .session
        .remove_prepared_trash(&reply.token)
        .unwrap_err()
        .contains("Trigger"));
    assert_eq!(
        f.catalog()
            .query_row("SELECT count(*) FROM external_files", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn trash_removal_rechecks_valid_file_count_changes_and_reused_numeric_package_ids() {
    for replace in [false, true] {
        let f = Fixture::new();
        let reply = f
            .session
            .prepare_trash_removal(&f.root, f.entry.clone())
            .unwrap();
        let c = f.catalog();
        if replace {
            let (name, path, source): (String, String, String) = c
                .query_row(
                    "SELECT name,relative_path,source_path FROM packages WHERE id=?1",
                    [f.entry.package_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            c.pragma_update(None, "foreign_keys", false).unwrap();
            c.execute("DELETE FROM packages WHERE id=?1", [f.entry.package_id])
                .unwrap();
            c.execute("INSERT INTO packages(id,name,relative_path,source_path) VALUES(?1,?2,'external/new-unique-package',?3)", rusqlite::params![f.entry.package_id,name,source]).unwrap();
            c.execute("UPDATE asset_files SET relative_path=replace(relative_path,?1,'external/new-unique-package') WHERE asset_id IN (SELECT id FROM assets WHERE package_id=?2)", rusqlite::params![path,f.entry.package_id]).unwrap();
            c.pragma_update(None, "foreign_keys", true).unwrap();
            assert_eq!(
                c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                0
            );
        } else {
            c.execute("INSERT INTO asset_files(asset_id,original_filename,relative_path,format,size_bytes) SELECT a.id,'Added.wav',p.relative_path || '/Added.wav','WAV',1 FROM assets a JOIN packages p ON p.id=a.package_id WHERE p.id=?1 LIMIT 1", [f.entry.package_id]).unwrap();
            c.execute("INSERT INTO external_files(file_id,identity,modified) VALUES(?1,'fixture','fixture')", [c.last_insert_rowid()]).unwrap();
        }
        assert!(f
            .session
            .remove_prepared_trash(&reply.token)
            .unwrap_err()
            .contains("nach der Vorbereitung verändert"));
    }
}

#[test]
fn trash_removal_real_foreign_key_failure_rolls_back_prior_deletes() {
    let f = Fixture::new();
    let before = f.hashes();
    let reply = f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .unwrap();
    let c = f.catalog();
    c.execute_batch("CREATE TABLE owned_blocker(file_id INTEGER REFERENCES asset_files(id));")
        .unwrap();
    c.execute("INSERT INTO owned_blocker SELECT f.id FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE a.package_id=?1 LIMIT 1", [f.entry.package_id]).unwrap();
    let error = f.session.remove_prepared_trash(&reply.token).unwrap_err();
    assert!(error.contains("FOREIGN KEY constraint failed"), "{error}");
    for (table, count) in [
        ("external_files", 4),
        ("asset_files", 5),
        ("assets", 5),
        ("packages", 3),
        ("external_sources", 2),
        ("trash", 2),
        ("audio_metadata", 2),
        ("asset_tags", 2),
        ("package_tags", 1),
        ("folder_assets", 2),
    ] {
        assert_eq!(
            c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            count,
            "{table}"
        );
    }
    assert_eq!(f.hashes(), before);
}

#[test]
fn trash_removal_tokens_are_server_generated_single_use_replaced_and_discarded_exactly() {
    let f = Fixture::new();
    let first = f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .unwrap();
    assert_eq!(first.token.len(), 36);
    assert_eq!(&first.token[14..15], "4");
    let second = f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .unwrap();
    assert_ne!(first.token, second.token);
    f.session.discard_trash_removal(&first.token).unwrap();
    assert!(f.session.remove_prepared_trash(&first.token).is_err());
    f.session.remove_prepared_trash(&second.token).unwrap();
    assert!(f.session.remove_prepared_trash(&second.token).is_err());
    let f = Fixture::new();
    let reply = f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .unwrap();
    f.session.discard_trash_removal(&reply.token).unwrap();
    assert!(f.session.remove_prepared_trash(&reply.token).is_err());
}

#[test]
fn trash_removal_root_same_and_a_b_a_switches_and_import_gate_busy_invalidate_execution() {
    for aba in [false, true] {
        let f = Fixture::new();
        let reply = f
            .session
            .prepare_trash_removal(&f.root, f.entry.clone())
            .unwrap();
        if aba {
            f.session.set_root(f.workspace.join("B")).unwrap();
        }
        f.session.set_root(f.root.clone()).unwrap();
        assert!(f.session.remove_prepared_trash(&reply.token).is_err());
    }
    let f = Fixture::new();
    let reply = f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .unwrap();
    let gate = f.session.import_gate.lock().unwrap();
    assert!(f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .is_err());
    assert!(f
        .session
        .remove_prepared_trash(&reply.token)
        .unwrap_err()
        .contains("Dateivorgang"));
    drop(gate);
    assert!(f.session.remove_prepared_trash(&reply.token).is_err());
    assert!(f
        .session
        .prepare_trash_removal(&f.workspace, f.entry.clone())
        .is_err());
}

#[test]
fn trash_removal_rejects_replaced_catalog_and_library_identities() {
    for replace_root in [false, true] {
        let f = Fixture::new();
        let reply = f
            .session
            .prepare_trash_removal(&f.root, f.entry.clone())
            .unwrap();
        if replace_root {
            let retained = f.workspace.join("RetainedLibrary");
            fs::rename(&f.root, &retained).unwrap();
            fs::create_dir(&f.root).unwrap();
            fs::copy(
                retained.join("catalog.sqlite3"),
                f.root.join("catalog.sqlite3"),
            )
            .unwrap();
        } else {
            fs::rename(
                f.root.join("catalog.sqlite3"),
                f.workspace.join("RetainedCatalog.sqlite3"),
            )
            .unwrap();
            fs::copy(
                f.workspace.join("RetainedCatalog.sqlite3"),
                f.root.join("catalog.sqlite3"),
            )
            .unwrap();
        }
        assert!(f
            .session
            .remove_prepared_trash(&reply.token)
            .unwrap_err()
            .contains("ersetzt"));
    }
}

#[test]
fn trash_removal_never_accesses_external_source_files() {
    let f = Fixture::new();
    let retained = f.workspace.join("OfflineOriginals");
    fs::rename(&f.source, &retained).unwrap();
    let reply = f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .unwrap();
    f.session.remove_prepared_trash(&reply.token).unwrap();
    assert!(!f.source.exists());
    assert_eq!(
        fs::read(retained.join("One.wav")).unwrap(),
        b"original sound"
    );
    assert_eq!(
        fs::read(retained.join("Two.ogg")).unwrap(),
        b"original variant"
    );
}

#[test]
fn trash_removal_rejects_noncanonical_external_package_paths() {
    let f = Fixture::new();
    let c = f.catalog();
    c.execute("UPDATE asset_files SET relative_path=replace(relative_path,'external/','external//') WHERE asset_id IN (SELECT id FROM assets WHERE package_id=?1)", [f.entry.package_id]).unwrap();
    c.execute("UPDATE packages SET relative_path=replace(relative_path,'external/','external//') WHERE id=?1", [f.entry.package_id]).unwrap();
    drop(c);
    assert!(f
        .session
        .prepare_trash_removal(&f.root, f.entry.clone())
        .is_err());
}
