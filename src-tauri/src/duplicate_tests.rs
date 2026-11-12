use super::*;
use crate::library::{CatalogQuery, Library};
use std::sync::atomic::{AtomicU64, Ordering};
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "file-duplicates-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn identical_contents_with_different_filenames_have_exact_evidence_and_keep_owned_images() {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    for (name, bytes) in [
        ("First.wav", b"abc"),
        ("Other.wav", b"abc"),
        ("Different.wav", b"abd"),
    ] {
        fs::write(source.join(name), bytes).unwrap();
    }
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let before = library.query(CatalogQuery::default()).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let database = fs::read(root.join("catalog.sqlite3")).unwrap();
    let flag = AtomicBool::new(false);
    let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
    assert_eq!(prepared.total(), 3);
    let report = run_duplicate_check(prepared, &flag, |_| {}).unwrap();
    assert!(report.complete, "{:?}", report.issues);
    assert_eq!(report.groups.len(), 1);
    assert_eq!(
        report.groups[0].sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        report.groups[0]
            .members
            .iter()
            .map(|id| report
                .files
                .iter()
                .find(|file| file.file_id == *id)
                .unwrap()
                .original_filename
                .as_str())
            .collect::<Vec<_>>(),
        vec!["First.wav", "Other.wav"]
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), database);
    let after = Library::open_read_only(&root)
        .unwrap()
        .query(CatalogQuery::default())
        .unwrap();
    assert_eq!(
        before.items.iter().map(|a| a.id).collect::<Vec<_>>(),
        after.items.iter().map(|a| a.id).collect::<Vec<_>>()
    );
    assert_eq!(fs::read(source.join("First.wav")).unwrap(), b"abc");
    assert_eq!(fs::read(source.join("Different.wav")).unwrap(), b"abd");
}

#[test]
fn stream_budget_is_charged_before_io_and_exhaustion_never_reports_complete() {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    for name in ["A.wav", "B.wav", "C.wav"] {
        fs::write(source.join(name), b"abc").unwrap();
    }
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let flag = AtomicBool::new(false);
    let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
    let report = run_with_limits(prepared, &flag, |_| {}, Limits { bytes: 5, ..LIMITS }).unwrap();
    assert!(!report.complete);
    assert_eq!(report.bytes_read, 5);
    assert_eq!(report.processed, 2);
    assert!(report.groups.is_empty());
    assert_eq!(report.files[2].status, "notChecked");
}

#[test]
fn zip_external_empty_and_multichunk_files_keep_annotations_trash_and_all_images_on_reopen() {
    use std::io::Write;
    let workspace = Workspace::new();
    let external = workspace.0.join("external");
    fs::create_dir(&external).unwrap();
    let payload = vec![b'a'; 1_000_000];
    fs::write(external.join("External.wav"), &payload).unwrap();
    fs::write(external.join("Empty.wav"), b"").unwrap();
    let archive = workspace.0.join("owned.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for (name, data) in [
        ("Owned.wav", payload.as_slice()),
        ("Zero.wav", b"".as_slice()),
        ("LICENSE.txt", b"license".as_slice()),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    let package = library.import_zip(&archive).unwrap();
    library.reference_folder(&external).unwrap();
    let before = library.query(CatalogQuery::default()).unwrap();
    let ids = before
        .items
        .iter()
        .flat_map(|a| a.files.iter().map(|f| f.id))
        .collect::<Vec<_>>();
    let project = library.create_folder("Owned project", None).unwrap();
    library
        .add_to_folder(project.id, before.items[0].id)
        .unwrap();
    library
        .annotate_asset(before.items[0].id, "Owned annotation", &["keep".into()])
        .unwrap();
    let annotated = library.query(CatalogQuery::default()).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let db = fs::read(root.join("catalog.sqlite3")).unwrap();
    let zip_before = fs::read(&archive).unwrap();
    let flag = AtomicBool::new(false);
    let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
    assert_eq!(prepared.total(), 4);
    let report = run_duplicate_check(prepared, &flag, |_| {}).unwrap();
    assert!(report.complete, "{:?}", report.issues);
    assert_eq!(report.groups.len(), 2);
    assert!(report.groups.iter().any(|g| g.size_bytes == 1_000_000
        && g.sha256 == "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        && g.members.len() == 2));
    assert!(report.groups.iter().any(|g| g.size_bytes == 0
        && g.sha256 == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), db);
    assert_eq!(fs::read(&archive).unwrap(), zip_before);
    assert_eq!(fs::read(external.join("External.wav")).unwrap(), payload);
    let mut library = Library::open(&root).unwrap();
    let mut reopened_ids = library
        .query(CatalogQuery::default())
        .unwrap()
        .items
        .iter()
        .flat_map(|a| a.files.iter().map(|f| f.id))
        .collect::<Vec<_>>();
    let mut ids = ids;
    reopened_ids.sort();
    ids.sort();
    assert_eq!(reopened_ids, ids);
    let after = library.query(CatalogQuery::default()).unwrap();
    assert_eq!(
        after
            .items
            .iter()
            .map(|a| (&a.name, &a.tags, &a.folder_ids))
            .collect::<Vec<_>>(),
        annotated
            .items
            .iter()
            .map(|a| (&a.name, &a.tags, &a.folder_ids))
            .collect::<Vec<_>>()
    );
    library.trash_package(package.id).unwrap();
    drop(library);
    let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
    assert_eq!(prepared.total(), 2);
    assert!(run_duplicate_check(prepared, &flag, |_| {})
        .unwrap()
        .groups
        .is_empty());
}

#[test]
fn same_size_restored_mtime_change_and_substituted_parent_cannot_publish_groups() {
    for parent in [false, true] {
        let workspace = Workspace::new();
        let source = workspace.0.join("source/nested");
        fs::create_dir_all(&source).unwrap();
        for name in ["A.wav", "B.wav"] {
            fs::write(source.join(name), vec![b'a'; 131072]).unwrap();
        }
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        library.import_folder(&workspace.0.join("source")).unwrap();
        let file = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
        let path = library.resolve_preview(file).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let root = library.root().to_owned();
        drop(library);
        let db = fs::read(root.join("catalog.sqlite3")).unwrap();
        let flag = AtomicBool::new(false);
        let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
        let mut changed = false;
        let report = run_duplicate_check(prepared, &flag, |p| {
            if p.stage == "hash" && p.bytes_read > 0 && !changed {
                changed = true;
                if parent {
                    let folder = path.parent().unwrap();
                    fs::rename(folder, folder.with_file_name("held-original")).unwrap();
                    fs::create_dir(folder).unwrap();
                    for name in ["A.wav", "B.wav"] {
                        fs::write(folder.join(name), vec![b'a'; 131072]).unwrap();
                    }
                } else {
                    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
                    fs::write(&path, vec![b'b'; 131072]).unwrap();
                    file.set_times(fs::FileTimes::new().set_modified(modified))
                        .unwrap();
                }
            }
        })
        .unwrap();
        assert!(changed);
        assert!(!report.complete);
        assert!(report.groups.is_empty());
        assert!(report.files.iter().any(|f| f.status == "unavailable"));
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), db);
        if !parent {
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
            assert_eq!(fs::read(&path).unwrap(), vec![b'b'; 131072]);
        }
    }
}

#[test]
fn physical_aliases_are_evidence_without_claiming_another_copy_and_cancel_discards_groups() {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    for name in ["A.wav", "B.wav"] {
        fs::write(source.join(name), b"abc").unwrap();
    }
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let files = library
        .query(CatalogQuery::default())
        .unwrap()
        .items
        .into_iter()
        .flat_map(|a| a.files)
        .collect::<Vec<_>>();
    let a = library.resolve_preview(files[0].id).unwrap();
    let b = library.resolve_preview(files[1].id).unwrap();
    fs::remove_file(&b).unwrap();
    fs::hard_link(&a, &b).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let flag = AtomicBool::new(false);
    let report = run_duplicate_check(
        prepare_duplicate_check(&root, None, &flag).unwrap(),
        &flag,
        |_| {},
    )
    .unwrap();
    assert!(report.complete);
    assert_eq!(report.groups.len(), 1);
    assert!(report.groups[0].shared_source);
    for stage in ["hash", "compare", "verify"] {
        flag.store(false, Ordering::Relaxed);
        let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
        let report = run_duplicate_check(prepared, &flag, |p| {
            if p.stage == stage {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
        assert!(report.cancelled);
        assert!(!report.complete);
        assert!(report.groups.is_empty());
    }
}

#[test]
fn javascript_unsafe_catalogue_ids_are_refused_without_changing_catalogue_bytes() {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("A.wav"), b"abc").unwrap();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let connection = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
    connection
        .execute("UPDATE asset_files SET id=9007199254740992", [])
        .unwrap();
    drop(connection);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    assert!(prepare_duplicate_check(&root, None, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
}

#[test]
fn forced_hash_candidate_collision_still_requires_matching_original_bytes() {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    for (name, data) in [
        ("A.wav", b"abc"),
        ("B.wav", b"abc"),
        ("C.wav", b"abd"),
        ("D.wav", b"abd"),
    ] {
        fs::write(source.join(name), data).unwrap();
    }
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let image = fs::read(root.join("catalog.sqlite3")).unwrap();
    let flag = AtomicBool::new(false);
    let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
    // Inject candidate-key collisions at the bucket boundary; SDK hashing and all real source reads still run.
    let report = run_with_candidate_key(prepared, &flag, |_| {}, LIMITS, |_| [0; 32]).unwrap();
    assert!(report.complete, "{:?}", report.issues);
    assert_eq!(report.groups.len(), 2);
    let mut names = report
        .groups
        .iter()
        .map(|g| {
            g.members
                .iter()
                .map(|id| {
                    report
                        .files
                        .iter()
                        .find(|f| f.file_id == *id)
                        .unwrap()
                        .original_filename
                        .as_str()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, vec![vec!["A.wav", "B.wav"], vec!["C.wav", "D.wav"]]);
    assert!(report
        .groups
        .iter()
        .any(|g| g.sha256 == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    assert_eq!(report.bytes_read, 36); // Four three-byte hashes plus four six-byte comparisons.
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), image);
}

#[test]
fn explicit_scope_includes_variants_beyond_the_catalogue_page_and_refuses_inactive_packages() {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    for i in 0..45 {
        fs::write(source.join(format!("Tone{i:03}.wav")), b"abc").unwrap();
    }
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    let package = library.import_folder(&source).unwrap();
    fs::write(workspace.0.join("second.wav"), b"abc").unwrap();
    let second = workspace.0.join("second");
    fs::create_dir(&second).unwrap();
    fs::write(second.join("Other.wav"), b"abc").unwrap();
    library.import_folder(&second).unwrap();
    assert_eq!(
        library.query(CatalogQuery::default()).unwrap().items.len(),
        40
    );
    let root = library.root().to_owned();
    drop(library);
    let flag = AtomicBool::new(false);
    let db = fs::read(root.join("catalog.sqlite3")).unwrap();
    for (scope, count) in [(Some(package.id), 45), (None, 46)] {
        let prepared = prepare_duplicate_check(&root, scope, &flag).unwrap();
        assert_eq!(prepared.total(), count);
        let report = run_duplicate_check(prepared, &flag, |_| {}).unwrap();
        assert!(report.complete, "{:?}", report.issues);
        assert_eq!(report.groups[0].members.len(), count);
    }
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), db);
    assert!(prepare_duplicate_check(&root, Some(9007199254740992), &flag).is_err());
    assert!(prepare_duplicate_check(&root, Some(9999), &flag).is_err());
    let mut library = Library::open(&root).unwrap();
    library.trash_package(package.id).unwrap();
    drop(library);
    assert!(prepare_duplicate_check(&root, Some(package.id), &flag).is_err());
}

#[test]
fn unavailable_external_changed_sources_symlinks_and_fifo_never_become_complete_evidence() {
    use std::os::unix::fs::symlink;
    for mode in ["offline", "changed", "legacy", "symlink", "fifo"] {
        let workspace = Workspace::new();
        let source = workspace.0.join("source");
        fs::create_dir(&source).unwrap();
        for name in ["A.wav", "B.wav"] {
            fs::write(source.join(name), b"abc").unwrap();
        }
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        library.reference_folder(&source).unwrap();
        let root = library.root().to_owned();
        drop(library);
        match mode {
            "offline" => fs::rename(&source, workspace.0.join("offline")).unwrap(),
            "changed" => fs::write(source.join("A.wav"), b"abd").unwrap(),
            "legacy" => {
                let c = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
                c.execute("UPDATE external_sources SET identity='legacy-device'", [])
                    .unwrap();
            }
            "symlink" => {
                fs::remove_file(source.join("A.wav")).unwrap();
                symlink(source.join("B.wav"), source.join("A.wav")).unwrap();
            }
            "fifo" => {
                fs::remove_file(source.join("A.wav")).unwrap();
                assert!(std::process::Command::new("/usr/bin/mkfifo")
                    .arg(source.join("A.wav"))
                    .status()
                    .unwrap()
                    .success());
            }
            _ => unreachable!(),
        }
        let db = fs::read(root.join("catalog.sqlite3")).unwrap();
        let flag = AtomicBool::new(false);
        let start = Instant::now();
        let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
        let report = run_duplicate_check(prepared, &flag, |_| {}).unwrap();
        assert!(!report.complete, "{mode}");
        assert!(report.groups.is_empty());
        assert!(
            report
                .files
                .iter()
                .any(|f| f.status == "unavailable" && f.reason.is_some()),
            "{mode}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "nonblocking refusal: {mode}"
        );
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), db);
        if mode != "offline" {
            assert_eq!(fs::read(source.join("B.wav")).unwrap(), b"abc");
        }
        assert!(!root.join("catalog.sqlite3-shm").exists());
        assert!(!root.join("catalog.sqlite3-wal").exists());
    }
}

#[test]
fn catalogue_reassignment_or_root_replacement_during_reads_discards_groups_and_preserves_writer_image(
) {
    for replace in [false, true] {
        let workspace = Workspace::new();
        let source = workspace.0.join("source");
        fs::create_dir(&source).unwrap();
        for name in ["A.wav", "B.wav"] {
            fs::write(source.join(name), b"abc").unwrap();
        }
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        library.import_folder(&source).unwrap();
        let root = library.root().to_owned();
        drop(library);
        let old = workspace.0.join("old");
        let image = fs::read(root.join("catalog.sqlite3")).unwrap();
        let flag = AtomicBool::new(false);
        let mut changed = false;
        let mut writer = None;
        let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
        let report=run_duplicate_check(prepared,&flag,|p| {if p.bytes_read>0 && !changed {
            changed=true;
            if replace {fs::rename(&root,&old).unwrap();fs::create_dir(&root).unwrap();fs::copy(old.join("catalog.sqlite3"),root.join("catalog.sqlite3")).unwrap();}
            else {let c=rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();c.execute("UPDATE asset_files SET original_filename='Writer.wav' WHERE id=(SELECT MIN(id) FROM asset_files)",[]).unwrap();drop(c);writer=Some(fs::read(root.join("catalog.sqlite3")).unwrap());}
        }}).unwrap();
        assert!(!report.complete);
        assert!(report.groups.is_empty());
        assert_eq!(
            fs::read(root.join("catalog.sqlite3")).unwrap(),
            writer.unwrap_or_else(|| image.clone())
        );
        if replace {
            assert_eq!(fs::read(old.join("catalog.sqlite3")).unwrap(), image);
        }
    }
}

#[test]
fn cancelled_empty_preparation_is_refused_before_a_snapshot_is_retained() {
    let workspace = Workspace::new();
    let root = workspace.0.join("Library");
    drop(Library::create(&root).unwrap());
    let image = fs::read(root.join("catalog.sqlite3")).unwrap();
    assert!(prepare_duplicate_check(&root, None, &AtomicBool::new(true)).is_err());
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), image);
}

#[test]
fn bounded_catalogue_row_text_sql_and_read_work_refuse_without_source_writes() {
    for mode in [
        "rows",
        "text",
        "length",
        "duplicate-id",
        "sql",
        "file",
        "time",
    ] {
        let workspace = Workspace::new();
        let source = workspace.0.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("A.wav"), b"abc").unwrap();
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        library.import_folder(&source).unwrap();
        let root = library.root().to_owned();
        drop(library);
        let c = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
        match mode {
            "rows" => {
                c.execute_batch("WITH RECURSIVE n(i) AS (VALUES(2) UNION ALL SELECT i+1 FROM n WHERE i<5001) INSERT INTO asset_files(id,asset_id,original_filename,relative_path,format,size_bytes) SELECT i,1,'A.wav', (SELECT relative_path FROM packages WHERE id=1) || '/A' || i || '.wav','wav',3 FROM n;").unwrap();
            }
            "text" | "length" => {
                c.execute(
                    "UPDATE packages SET name=?1",
                    ["x".repeat(if mode == "text" { 4097 } else { 65536 })],
                )
                .unwrap();
            }
            "duplicate-id" => {
                c.execute_batch("DROP VIEW active_asset_files; CREATE VIEW active_asset_files AS SELECT * FROM asset_files UNION ALL SELECT * FROM asset_files;").unwrap();
            }
            "sql" => {
                c.execute_batch("DROP VIEW active_asset_files; CREATE VIEW active_asset_files AS SELECT * FROM asset_files WHERE id IN (WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<10000000) SELECT max(i) FROM n);").unwrap();
            }
            "file" => {
                c.execute("UPDATE asset_files SET size_bytes=4294967297", [])
                    .unwrap();
            }
            "time" => {}
            _ => unreachable!(),
        }
        drop(c);
        let image = fs::read(root.join("catalog.sqlite3")).unwrap();
        let flag = AtomicBool::new(false);
        let start = Instant::now();
        match mode {
            "sql" => {
                let reader = super::super::storage::CatalogReader::open(&root).unwrap();
                assert!(snapshot(
                    &reader,
                    None,
                    &flag,
                    Limits {
                        work: Duration::from_millis(5),
                        ..LIMITS
                    },
                    Instant::now()
                )
                .is_err());
            }
            "file" | "time" => {
                let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
                let report = run_with_limits(
                    prepared,
                    &flag,
                    |_| {},
                    if mode == "time" {
                        Limits {
                            work: Duration::ZERO,
                            ..LIMITS
                        }
                    } else {
                        LIMITS
                    },
                )
                .unwrap();
                assert!(!report.complete);
                assert!(report.groups.is_empty());
                assert_eq!(report.bytes_read, 0);
            }
            _ => {
                let error = prepare_duplicate_check(&root, None, &flag)
                    .err()
                    .expect(mode);
                if mode == "rows" {
                    assert!(error.contains("5.000"), "{error}");
                }
                if mode == "duplicate-id" {
                    assert!(error.contains("mehrfach"), "{error}");
                }
            }
        }
        assert!(start.elapsed() < Duration::from_secs(2), "{mode}");
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), image);
        assert_eq!(fs::read(source.join("A.wav")).unwrap(), b"abc");
    }
}

#[test]
fn immutable_qualified_caller_never_creates_or_recovers_sidecars() {
    fn images(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_type().unwrap().is_file())
            .map(|entry| {
                (
                    entry.file_name().to_str().unwrap().to_owned(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }
    for mode in ["wal", "hot", "inactive"] {
        let workspace = Workspace::new();
        let source = workspace.0.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("A.wav"), b"abc").unwrap();
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        library.import_folder(&source).unwrap();
        let root = library.root().to_owned();
        drop(library);
        let mut connection = None;
        if mode == "wal" {
            let c = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
            c.execute_batch("PRAGMA journal_mode=WAL;UPDATE packages SET name='Owned WAL';")
                .unwrap();
            connection = Some(c);
        }
        if mode == "hot" {
            let mut header = vec![0; 1024];
            header[..8].copy_from_slice(&[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]);
            fs::write(root.join("catalog.sqlite3-journal"), header).unwrap();
        }
        if mode == "inactive" {
            let c = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
            c.execute_batch(
                "PRAGMA journal_mode=PERSIST;UPDATE packages SET name='Owned PERSIST';",
            )
            .unwrap();
            drop(c);
            assert!(root.join("catalog.sqlite3-journal").exists());
        }
        let before = images(&root);
        let flag = AtomicBool::new(false);
        let prepared = prepare_duplicate_check(&root, None, &flag);
        if mode == "inactive" {
            assert!(
                run_duplicate_check(prepared.unwrap(), &flag, |_| {})
                    .unwrap()
                    .complete
            );
        } else {
            assert!(prepared.is_err(), "{mode}");
        }
        assert_eq!(images(&root), before, "{mode}");
        assert_eq!(fs::read(source.join("A.wav")).unwrap(), b"abc");
        drop(connection);
    }
}

#[test]
fn interrupted_owned_read_checks_stop_before_retrying_the_real_file() {
    struct Interrupted<'a> {
        file: fs::File,
        flag: &'a AtomicBool,
        calls: usize,
    }
    impl Read for Interrupted<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                self.flag.store(true, Ordering::Relaxed);
                Err(std::io::ErrorKind::Interrupted.into())
            } else {
                self.file.read(buffer)
            }
        }
    }
    let workspace = Workspace::new();
    let path = workspace.0.join("owned.bin");
    fs::write(&path, b"abc").unwrap();
    let flag = AtomicBool::new(false);
    let mut file = Interrupted {
        file: fs::File::open(&path).unwrap(),
        flag: &flag,
        calls: 0,
    };
    let mut budget = Budget {
        flag: &flag,
        started: Instant::now(),
        limits: LIMITS,
        bytes: 0,
        exhausted: false,
        processed: 0,
        total: 1,
        stage: "hash",
        last: Instant::now(),
        progress: |_| {},
    };
    assert!(budget.read(&mut file, &mut [0; 3]).is_err());
    assert_eq!(file.calls, 1);
    assert_eq!(budget.bytes, 0);
    assert_eq!(fs::read(path).unwrap(), b"abc");
}

#[test]
fn prepared_managed_file_or_parent_replacement_is_refused_before_first_hash() {
    for parent in [false, true] {
        let workspace = Workspace::new();
        let source = workspace.0.join("source/nested");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("A.wav"), b"abc").unwrap();
        fs::write(source.join("B.wav"), b"abd").unwrap();
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        library.import_folder(&workspace.0.join("source")).unwrap();
        let file = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .iter()
            .flat_map(|a| a.files.iter())
            .find(|f| f.original_filename == "A.wav")
            .unwrap()
            .id;
        let path = library.resolve_preview(file).unwrap();
        let root = library.root().to_owned();
        drop(library);
        let image = fs::read(root.join("catalog.sqlite3")).unwrap();
        let flag = AtomicBool::new(false);
        let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
        if parent {
            let directory = path.parent().unwrap();
            fs::rename(directory, directory.with_file_name("original-held")).unwrap();
            fs::create_dir(directory).unwrap();
            fs::write(directory.join("A.wav"), b"abd").unwrap();
            fs::write(directory.join("B.wav"), b"abd").unwrap();
        } else {
            fs::rename(&path, path.with_file_name("Original.wav")).unwrap();
            fs::write(&path, b"abd").unwrap();
        }
        let report = run_duplicate_check(prepared, &flag, |_| {}).unwrap();
        assert!(!report.complete, "replacement must invalidate preparation");
        assert!(report.groups.is_empty());
        assert_eq!(report.bytes_read, 0);
        assert!(report.files.iter().all(|f| f.status == "unavailable"));
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), image);
        assert_eq!(fs::read(path).unwrap(), b"abd");
    }
}

#[test]
fn initially_missing_or_blocked_file_cannot_be_adopted_after_preparation() {
    use std::os::unix::fs::symlink;
    for blocked in [false, true] {
        let workspace = Workspace::new();
        let source = workspace.0.join("source");
        fs::create_dir(&source).unwrap();
        for name in ["A.wav", "B.wav"] {
            fs::write(source.join(name), b"abc").unwrap();
        }
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        library.import_folder(&source).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        let path = library.resolve_preview(page.items[0].files[0].id).unwrap();
        let root = library.root().to_owned();
        drop(library);
        fs::remove_file(&path).unwrap();
        if blocked {
            symlink(source.join("A.wav"), &path).unwrap();
        }
        let image = fs::read(root.join("catalog.sqlite3")).unwrap();
        let flag = AtomicBool::new(false);
        let prepared = prepare_duplicate_check(&root, None, &flag).unwrap();
        assert_eq!(prepared.total(), 2);
        if blocked {
            fs::remove_file(&path).unwrap();
        }
        fs::write(&path, b"abc").unwrap();
        let report = run_duplicate_check(prepared, &flag, |_| {}).unwrap();
        assert!(
            !report.complete,
            "restoration must require a new preparation"
        );
        assert!(report.groups.is_empty());
        assert!(report
            .files
            .iter()
            .any(|f| f.status == "unavailable" && f.reason.is_some()));
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), image);
        assert_eq!(fs::read(path).unwrap(), b"abc");
    }
}

#[test]
fn final_snapshot_keeps_the_original_expired_run_deadline_and_drops_sql_guard_on_refusal() {
    let workspace = Workspace::new();
    let source = workspace.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("A.wav"), b"").unwrap();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let image = fs::read(root.join("catalog.sqlite3")).unwrap();
    let reader = super::super::storage::CatalogReader::open(&root).unwrap();
    let flag = AtomicBool::new(false);
    let deadline_start = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    let error = snapshot(
        &reader,
        None,
        &flag,
        Limits {
            work: Duration::from_millis(10),
            ..LIMITS
        },
        deadline_start,
    )
    .err()
    .expect("an expired run deadline cannot be renewed for final SQL");
    assert!(error.contains("Zeitlimit"));
    assert!(start.elapsed() < Duration::from_secs(1));
    // A refused snapshot leaves no borrowed callback behind and grants no additional read-only lease.
    assert_eq!(
        reader
            .library
            .connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), image);
}
