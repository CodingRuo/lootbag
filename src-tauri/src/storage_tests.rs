use super::*;
use crate::library::CatalogQuery;
use std::sync::atomic::{AtomicU64, Ordering};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "storage-report-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn source(&self) -> PathBuf {
        let source = self.0.join("Originals");
        fs::create_dir(&source).unwrap();
        for (name, bytes) in [
            ("A.wav", b"sound".as_slice()),
            ("LICENSE.txt", b"license".as_slice()),
            ("mesh.mtl", b"mtl".as_slice()),
            ("buffer.bin", b"bin".as_slice()),
        ] {
            fs::write(source.join(name), bytes).unwrap();
        }
        source
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn report(library: &Library) -> StorageReport {
    read_storage_report(library.root(), &AtomicBool::new(false), |_| {}).unwrap()
}
fn category(report: &StorageReport, name: &str) -> u64 {
    report
        .categories
        .iter()
        .find(|item| item.category == name)
        .unwrap()
        .bytes
}

#[test]
fn storage_report_counts_owned_companions_once_and_preserves_catalogue_and_originals() {
    let workspace = Workspace::new();
    let source = workspace.source();
    let root = workspace.0.join("Library");
    let mut library = Library::create(&root).unwrap();
    library.import_folder(&source).unwrap();
    let asset_id = library.query(CatalogQuery::default()).unwrap().items[0].id;
    for name in ["Project A", "Project B"] {
        let folder = library.create_folder(name, None).unwrap();
        library.add_to_folder(folder.id, asset_id).unwrap();
    }
    drop(library);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let original = fs::read(source.join("A.wav")).unwrap();
    let library = Library::open_read_only(&root).unwrap();
    let result = report(&library);
    assert!(result.complete, "{:?}", result.issues);
    assert_eq!(category(&result, "managed"), 18);
    assert_eq!(category(&result, "trash"), 0);
    assert_eq!(category(&result, "support"), before.len() as u64);
    assert_eq!(result.total_bytes, before.len() as u64 + 18);
    assert_eq!(
        result.categories.iter().map(|item| item.bytes).sum::<u64>(),
        result.total_bytes
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert_eq!(fs::read(source.join("A.wav")).unwrap(), original);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
}

#[test]
fn storage_report_trash_is_retained_and_overlapping_package_and_file_trash_never_double_count() {
    let workspace = Workspace::new();
    let source = workspace.source();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    let package = library.import_folder(&source).unwrap();
    let file_id = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    library.trash_file(file_id).unwrap();
    assert_eq!(category(&report(&library), "trash"), 5);
    assert_eq!(category(&report(&library), "managed"), 13);
    library.trash_package(package.id).unwrap();
    assert_eq!(category(&report(&library), "trash"), 18);
    assert_eq!(category(&report(&library), "managed"), 0);
    let package_trash = library
        .trash_entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.file_id.is_none())
        .unwrap();
    library.restore_trash(package_trash.id).unwrap();
    assert_eq!(category(&report(&library), "trash"), 5);
    let file_trash = library
        .trash_entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.file_id.is_some())
        .unwrap();
    library.restore_trash(file_trash.id).unwrap();
    assert_eq!(category(&report(&library), "managed"), 18);
    assert_eq!(category(&report(&library), "trash"), 0);
}

#[test]
fn storage_report_descriptor_opener_is_pinned_and_never_creates_wal_sidecars_or_backups() {
    use std::os::fd::AsRawFd;
    let workspace = Workspace::new();
    let root = workspace.0.join("Library");
    let library = Library::create(&root).unwrap();
    drop(library);
    let root = root.canonicalize().unwrap();
    let parent = anchored::root(&root).unwrap();
    let held = anchored::child(&parent, std::ffi::OsStr::new("catalog.sqlite3")).unwrap();
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    assert!(held.as_raw_fd() >= 0);
    fs::rename(
        root.join("catalog.sqlite3"),
        root.join("pinned-original.sqlite3"),
    )
    .unwrap();
    let mut replacement = before.clone();
    replacement[18] = 2;
    replacement[19] = 2;
    replacement[60..64].copy_from_slice(&99_i32.to_be_bytes());
    fs::write(root.join("catalog.sqlite3"), &replacement).unwrap();
    let files_before = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<HashSet<_>>();
    let pinned = immutable_catalog(&root, &held).unwrap();
    assert_eq!(
        pinned
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(pinned.connection.execute("INSERT INTO packages(name,relative_path,source_path) VALUES ('write','packages/write','original')", []).is_err());
    assert!(read_storage_report(&root, &AtomicBool::new(false), |_| {})
        .unwrap_err()
        .contains("Rollback"));
    drop(pinned);
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), replacement);
    assert_eq!(
        fs::read(root.join("pinned-original.sqlite3")).unwrap(),
        before
    );
    assert_eq!(
        fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<HashSet<_>>(),
        files_before
    );
    assert!(!root.join("catalog.sqlite3-wal").exists());
    assert!(!root.join("catalog.sqlite3-shm").exists());
}

#[test]
fn storage_report_zip_and_external_versions_remain_distinct_from_library_payload_and_offline_sources(
) {
    use std::io::Write;
    let workspace = Workspace::new();
    let source = workspace.source();
    let archive = workspace.0.join("Package.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for (name, value) in [
        ("Model.obj", b"model".as_slice()),
        ("Model.mtl", b"mtl".as_slice()),
        ("LICENSE.txt", b"license".as_slice()),
        ("buffer.bin", b"bin".as_slice()),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(value).unwrap();
    }
    zip.finish().unwrap();
    let archive_before = fs::read(&archive).unwrap();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_zip(&archive).unwrap();
    let first = library.reference_folder(&source).unwrap();
    library.reference_folder(&source).unwrap();
    library.trash_package(first.id).unwrap();
    let result = report(&library);
    assert!(result.complete, "{:?}", result.issues);
    assert_eq!(category(&result, "managed"), 18);
    assert_eq!(result.external.as_ref().unwrap().recorded_bytes, 5);
    assert_eq!(result.external.as_ref().unwrap().references, 2);
    assert_eq!(result.external.as_ref().unwrap().recorded_versions, 1);
    assert_eq!(result.external.as_ref().unwrap().shared_versions, 1);
    fs::write(source.join("A.wav"), b"new sound").unwrap();
    library.reference_folder(&source).unwrap();
    fs::rename(&source, workspace.0.join("Offline source")).unwrap();
    let result = report(&library);
    assert!(result.complete, "{:?}", result.issues);
    assert_eq!(result.external.as_ref().unwrap().recorded_bytes, 14);
    assert_eq!(result.external.as_ref().unwrap().recorded_versions, 2);
    assert_eq!(result.external.as_ref().unwrap().references, 3);
    assert_eq!(result.external.as_ref().unwrap().active_versions, 1);
    assert_eq!(result.external.as_ref().unwrap().shared_versions, 1);
    assert_eq!(category(&result, "managed"), 18);
    assert_eq!(fs::read(&archive).unwrap(), archive_before);
    assert_eq!(
        fs::read(workspace.0.join("Offline source/A.wav")).unwrap(),
        b"new sound"
    );
}

#[test]
fn storage_report_includes_sidecars_repair_backup_staging_unknown_raw_names_and_sparse_logical_size(
) {
    use std::os::unix::ffi::OsStringExt;
    let workspace = Workspace::new();
    let library = Library::create(&workspace.0.join("Library")).unwrap();
    let root = library.root();
    let catalog_size = fs::metadata(root.join("catalog.sqlite3")).unwrap().len();
    for (name, value) in [
        (
            "catalog-before-macos-metadata-owned-1.sqlite3",
            b"backup".as_slice(),
        ),
        ("catalog.sqlite3-journal", b"journal".as_slice()),
        ("catalog.sqlite3-wal", b"wal".as_slice()),
        ("catalog.sqlite3-shm", b"shm".as_slice()),
        ("unknown.bin", b"unknown".as_slice()),
    ] {
        fs::write(root.join(name), value).unwrap();
    }
    fs::create_dir_all(root.join("packages/.import-owned")).unwrap();
    fs::write(root.join("packages/.import-owned/staged.bin"), b"stage").unwrap();
    fs::write(
        root.join(std::ffi::OsString::from_vec(
            "raw-é.bin".as_bytes().to_vec(),
        )),
        b"raw",
    )
    .unwrap();
    let sparse = fs::File::create(root.join("sparse.bin")).unwrap();
    sparse.set_len(1_048_576).unwrap();
    drop(sparse);
    let result = report(&library);
    assert!(result.complete, "{:?}", result.issues);
    assert_eq!(category(&result, "support"), catalog_size + 19);
    assert_eq!(category(&result, "staging"), 5);
    assert_eq!(category(&result, "unclassified"), 1_048_586);
    assert_eq!(result.total_bytes, catalog_size + 1_048_610);
}

#[test]
fn storage_report_hardlinks_count_once_and_cross_category_or_outside_names_are_not_reclaimable() {
    let workspace = Workspace::new();
    let source = workspace.source();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let file = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    let managed = library.resolve_preview(file).unwrap();
    fs::hard_link(&managed, managed.with_file_name("same-category-copy.bin")).unwrap();
    let result = report(&library);
    assert!(result.complete, "{:?}", result.issues);
    assert_eq!(category(&result, "managed"), 18);
    assert_eq!(
        result
            .categories
            .iter()
            .find(|item| item.category == "managed")
            .unwrap()
            .paths,
        5
    );
    library.trash_file(file).unwrap();
    let result = report(&library);
    assert!(result.complete, "{:?}", result.issues);
    assert_eq!(category(&result, "managed"), 13);
    assert_eq!(category(&result, "trash"), 0);
    assert_eq!(category(&result, "shared"), 5);
    fs::hard_link(
        &managed,
        library
            .root()
            .join("catalog-before-macos-metadata-shared-1.sqlite3"),
    )
    .unwrap();
    fs::hard_link(&managed, workspace.0.join("Outside library.bin")).unwrap();
    let result = report(&library);
    assert!(!result.complete);
    assert_eq!(category(&result, "shared"), 5);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("weitere Hardlinks")));
    assert_eq!(
        fs::read(workspace.0.join("Outside library.bin")).unwrap(),
        b"sound"
    );
}

#[test]
fn storage_report_excludes_symlink_escape_fifo_and_missing_known_files_without_blocking() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    extern "C" {
        fn mkfifo(path: *const std::ffi::c_char, mode: u16) -> i32;
    }
    let workspace = Workspace::new();
    let source = workspace.source();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let file = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    fs::remove_file(library.resolve_preview(file).unwrap()).unwrap();
    fs::write(workspace.0.join("outside.bin"), b"outside").unwrap();
    symlink(
        workspace.0.join("outside.bin"),
        library.root().join("escape.bin"),
    )
    .unwrap();
    symlink(&source, library.root().join("escape-directory")).unwrap();
    let fifo = CString::new(library.root().join("pipe").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let started = Instant::now();
    let result = report(&library);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!result.complete);
    assert_eq!(result.excluded_entries, 3);
    assert_eq!(category(&result, "managed"), 13);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("Katalogisierte Datei nicht gemessen")));
    assert_eq!(
        fs::read(workspace.0.join("outside.bin")).unwrap(),
        b"outside"
    );
}

#[test]
fn storage_report_marks_file_directory_catalogue_drift_and_root_replacement_incomplete() {
    let workspace = Workspace::new();
    let source = workspace.source();
    let mut writer = Library::create(&workspace.0.join("Library")).unwrap();
    let package = writer.import_folder(&source).unwrap();
    let file = writer.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
    let managed = writer.resolve_preview(file).unwrap();
    let root = writer.root().to_owned();
    let reader = Library::open_read_only(&root).unwrap();
    let mut changed = false;
    let result = reader
        .storage_report(&AtomicBool::new(false), |progress| {
            if progress.stage == "verify" && !changed {
                changed = true;
                fs::write(&managed, b"changed payload").unwrap();
                fs::write(managed.with_file_name("new-companion.bin"), b"new").unwrap();
                writer.trash_package(package.id).unwrap();
            }
        })
        .unwrap();
    assert!(!result.complete);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("Katalogzuordnung")));
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("Datei wurde inzwischen")));
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("Ordner wurde inzwischen")));
    let reader = Library::open_read_only(&root).unwrap();
    let mut replaced = false;
    let result = reader
        .storage_report(&AtomicBool::new(false), |progress| {
            if progress.stage == "verify" && !replaced {
                replaced = true;
                fs::rename(&root, workspace.0.join("Moved library")).unwrap();
                fs::create_dir(&root).unwrap();
                fs::copy(
                    workspace.0.join("Moved library/catalog.sqlite3"),
                    root.join("catalog.sqlite3"),
                )
                .unwrap();
            }
        })
        .unwrap();
    assert!(!result.complete);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("Bibliotheksordner")));
}

#[test]
fn storage_report_cancel_and_work_entry_depth_name_row_and_arithmetic_limits_never_report_success()
{
    let workspace = Workspace::new();
    let source = workspace.source();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.import_folder(&source).unwrap();
    let cancelled = AtomicBool::new(false);
    let result = library
        .storage_report(&cancelled, |progress| {
            if progress.stage == "walk" {
                cancelled.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
    assert!(result.cancelled);
    assert!(!result.complete);
    assert_eq!(result.total_bytes, 0);
    for limits in [
        Limits {
            entries: 1,
            ..LIMITS
        },
        Limits { depth: 0, ..LIMITS },
        Limits {
            work: Duration::ZERO,
            ..LIMITS
        },
    ] {
        let result = library
            .storage_report_with_limits(&AtomicBool::new(false), |_| {}, limits)
            .unwrap();
        assert!(!result.complete);
    }
    assert!(library
        .storage_report_with_limits(
            &AtomicBool::new(false),
            |_| {},
            Limits { rows: 0, ..LIMITS }
        )
        .is_err());
    assert!(library
        .storage_report_with_limits(
            &AtomicBool::new(false),
            |_| {},
            Limits {
                retained_bytes: 1,
                ..LIMITS
            }
        )
        .is_err());
    assert!(add(MAX_SAFE_BYTES, 1).is_err());
    assert!(add(u64::MAX, 1).is_err());
}

#[test]
fn storage_report_raw_dirent_names_are_bounded_and_never_lossily_merged() {
    use std::os::unix::ffi::OsStrExt;
    let raw = anchored::decode_name(b"raw-\xff.bin").unwrap();
    assert_eq!(raw.as_bytes(), b"raw-\xff.bin");
    assert_ne!(raw, std::ffi::OsString::from("raw-�.bin"));
    for bytes in [
        b"".as_slice(),
        b"bad/name".as_slice(),
        b"bad\0name".as_slice(),
    ] {
        assert!(anchored::decode_name(bytes).is_err());
    }
    assert!(anchored::decode_name(&vec![b'a'; 1024]).is_err());
    assert!(qualified_identity(
        "mac-volume:0123456789abcdef0123456789abcdef:12"
    ));
    assert!(!qualified_identity("1234:12"));
    assert!(!qualified_identity(
        "mac-volume:01234567-89ab-cdef-0123-456789abcdef:12"
    ));
}

#[test]
fn storage_report_unqualified_external_versions_warn_and_are_never_guessed_into_one_file() {
    let workspace = Workspace::new();
    let source = workspace.source();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    library.reference_folder(&source).unwrap();
    library.reference_folder(&source).unwrap();
    library
        .connection
        .execute(
            "UPDATE external_files SET identity='legacy:unknown',modified='unqualified-time'",
            [],
        )
        .unwrap();
    let result = report(&library);
    assert!(!result.complete);
    let external = result.external.unwrap();
    assert_eq!(external.recorded_bytes, 10);
    assert_eq!(external.recorded_versions, 2);
    assert_eq!(external.unqualified_references, 2);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("Identitätsversionen")));
}

#[test]
fn storage_report_rejects_oversized_sqlite_fields_before_column_materialization() {
    let workspace = Workspace::new();
    let source = workspace.source();
    let mut library = Library::create(&workspace.0.join("Library")).unwrap();
    let package = library.import_folder(&source).unwrap();
    library
        .connection
        .execute(
            "UPDATE packages SET relative_path=?1 WHERE id=?2",
            rusqlite::params!["x".repeat(40_000), package.id],
        )
        .unwrap();
    drop(library);
    let error = read_storage_report(
        &workspace.0.join("Library").canonicalize().unwrap(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert!(error.contains("too big"), "{error}");
}

#[test]
fn storage_report_sql_vm_budget_interrupts_work_and_unregisters_on_every_return() {
    let workspace = Workspace::new();
    let library = Library::create(&workspace.0.join("Library")).unwrap();
    let held = fs::File::open(library.root().join("catalog.sqlite3")).unwrap();
    let reader = immutable_catalog(library.root(), &held).unwrap();
    let long_query = "WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<1000000000) SELECT sum(n) FROM x";
    let cancelled = AtomicBool::new(false);
    let started = Instant::now();
    {
        let _budget = QueryBudget::install(
            &reader.connection,
            &cancelled,
            started,
            Duration::from_millis(5),
        );
        let error = reader
            .connection
            .query_row(long_query, [], |row| row.get::<_, i64>(0))
            .unwrap_err();
        assert!(
            matches!(error, rusqlite::Error::SqliteFailure(error, _) if error.code == rusqlite::ErrorCode::OperationInterrupted)
        );
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        reader
            .connection
            .query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        42
    );
    {
        let _budget = QueryBudget::install(
            &reader.connection,
            &cancelled,
            Instant::now(),
            Duration::from_secs(30),
        );
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(5));
                cancelled.store(true, Ordering::Relaxed);
            });
            let error = reader
                .connection
                .query_row(long_query, [], |row| row.get::<_, i64>(0))
                .unwrap_err();
            assert!(
                matches!(error, rusqlite::Error::SqliteFailure(error, _) if error.code == rusqlite::ErrorCode::OperationInterrupted)
            );
        });
    }
    assert_eq!(
        reader
            .connection
            .query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        42
    );
}

#[test]
fn storage_report_hot_rollback_journal_child() {
    let Some(path) = std::env::var_os("STORAGE_REPORT_OWNED_CRASH_DATABASE") else {
        return;
    };
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=1; CREATE TABLE owned_spill(data BLOB); BEGIN IMMEDIATE; INSERT INTO trash(package_id) VALUES(1);")
        .unwrap();
    for _ in 0..64 {
        connection
            .execute("INSERT INTO owned_spill VALUES(zeroblob(4096))", [])
            .unwrap();
    }
    // Simulate a crashed writer without running SQLite's connection rollback destructor.
    std::process::exit(0);
}

#[test]
fn storage_report_rejects_actual_hot_rollback_journal_without_recovery_writes() {
    let workspace = Workspace::new();
    let source = workspace.source();
    let root = workspace.0.join("Library");
    let mut library = Library::create(&root).unwrap();
    library.import_folder(&source).unwrap();
    drop(library);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "library::storage::tests::storage_report_hot_rollback_journal_child",
            "--exact",
        ])
        .env(
            "STORAGE_REPORT_OWNED_CRASH_DATABASE",
            root.join("catalog.sqlite3"),
        )
        .output()
        .unwrap();
    assert!(status.status.success(), "{:?}", status);
    let database = fs::read(root.join("catalog.sqlite3")).unwrap();
    let journal = fs::read(root.join("catalog.sqlite3-journal")).unwrap();
    assert!(journal.len() > 512);
    assert_eq!(
        &journal[..8],
        &[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]
    );
    let names = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<HashSet<_>>();
    let error = read_storage_report(
        &root.canonicalize().unwrap(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert!(error.contains("Rollback-Journal"), "{error}");
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), database);
    assert_eq!(
        fs::read(root.join("catalog.sqlite3-journal")).unwrap(),
        journal
    );
    assert_eq!(fs::read(source.join("A.wav")).unwrap(), b"sound");
    assert_eq!(
        fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<HashSet<_>>(),
        names
    );
    let copy = workspace.0.join("recovery-copy.sqlite3");
    fs::write(&copy, &database).unwrap();
    fs::write(workspace.0.join("recovery-copy.sqlite3-journal"), &journal).unwrap();
    let recovered = rusqlite::Connection::open(copy).unwrap();
    assert_eq!(
        recovered
            .query_row("SELECT count(*) FROM trash", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0,
        "only the owned copy is recovered, proving original main pages were uncommitted"
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), database);
    assert_eq!(
        fs::read(root.join("catalog.sqlite3-journal")).unwrap(),
        journal
    );
}

#[test]
fn storage_report_counts_inactive_persist_journal_and_rechecks_activation() {
    let workspace = Workspace::new();
    let root = workspace.0.join("Library");
    let library = Library::create(&root).unwrap();
    drop(library);
    let writer = rusqlite::Connection::open(root.join("catalog.sqlite3")).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=PERSIST; INSERT INTO folders(name) VALUES('Owned journal fixture')")
        .unwrap();
    drop(writer);
    let database = fs::read(root.join("catalog.sqlite3")).unwrap();
    let inactive = fs::read(root.join("catalog.sqlite3-journal")).unwrap();
    assert!(inactive.len() > 512);
    assert_eq!(&inactive[..8], &[0; 8]);
    let root = root.canonicalize().unwrap();
    let result = read_storage_report(&root, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(result.complete, "{:?}", result.issues);
    assert_eq!(
        category(&result, "support"),
        (database.len() + inactive.len()) as u64
    );
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), database);
    assert_eq!(
        fs::read(root.join("catalog.sqlite3-journal")).unwrap(),
        inactive
    );
    let mut activated = inactive.clone();
    activated[..8].copy_from_slice(&[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]);
    let mut changed = false;
    let result = read_storage_report(&root, &AtomicBool::new(false), |progress| {
        if progress.stage == "walk" && !changed {
            fs::write(root.join("catalog.sqlite3-journal"), &activated).unwrap();
            changed = true;
        }
    })
    .unwrap();
    assert!(changed);
    assert!(!result.complete);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.contains("Rollback-Journal")));
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), database);
    assert_eq!(
        fs::read(root.join("catalog.sqlite3-journal")).unwrap(),
        activated
    );
}
