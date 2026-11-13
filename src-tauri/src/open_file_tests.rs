// "Öffnen" and "Im Finder zeigen" hand `resolve_preview` results to the OS; these pin what that resolver returns.
use super::*;
use std::io::Write;

fn workspace(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "open-file-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    dunce::canonicalize(root).unwrap()
}

const NAME: &str = "Grüne Wiese 🌿 Schritt.wav";

fn source(workspace: &Path) -> PathBuf {
    let source = workspace.join("Mein Paket ä");
    fs::create_dir_all(source.join("Ordner mit Leerzeichen")).unwrap();
    fs::write(source.join("Ordner mit Leerzeichen").join(NAME), b"step").unwrap();
    source
}

fn only_file(library: &Library) -> i64 {
    library.query(CatalogQuery::default()).unwrap().items[0].files[0].id
}

#[test]
fn managed_files_with_spaces_and_unicode_resolve_inside_the_library() {
    let workspace = workspace("managed");
    let mut library = Library::create(&workspace.join("library")).unwrap();
    library.import_folder(&source(&workspace)).unwrap();
    let id = only_file(&library);

    let path = library.resolve_preview(id).unwrap();
    assert!(path.starts_with(workspace.join("library/packages")));
    assert_eq!(fs::read(&path).unwrap(), b"step");

    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(
        workspace
            .join("Mein Paket ä/Ordner mit Leerzeichen")
            .join(NAME),
        &path,
    )
    .unwrap();
    let error = library.resolve_preview(id).unwrap_err();
    assert!(error.contains("symbolische Verknüpfung"), "{error}");

    fs::remove_file(&path).unwrap();
    let error = library.resolve_preview(id).unwrap_err();
    assert!(error.starts_with("Asset-Datei nicht verfügbar"), "{error}");
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn referenced_files_resolve_to_the_original_and_reject_symlinks_and_offline_sources() {
    let workspace = workspace("referenced");
    let source = source(&workspace);
    let original = source.join("Ordner mit Leerzeichen").join(NAME);
    let mut library = Library::create(&workspace.join("library")).unwrap();
    library.reference_folder(&source).unwrap();
    let id = only_file(&library);

    assert_eq!(library.resolve_preview(id).unwrap(), original);

    fs::rename(&original, workspace.join("moved.wav")).unwrap();
    std::os::unix::fs::symlink(workspace.join("moved.wav"), &original).unwrap();
    let error = library.resolve_preview(id).unwrap_err();
    assert!(error.contains("symbolische Verknüpfung"), "{error}");

    fs::rename(&source, workspace.join("offline")).unwrap();
    let error = library.resolve_preview(id).unwrap_err();
    assert!(
        error.starts_with("Externe Quelle nicht verfügbar"),
        "{error}"
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn trashed_files_and_packages_are_not_resolved() {
    let workspace = workspace("trashed");
    let mut library = Library::create(&workspace.join("library")).unwrap();
    let package = library.import_folder(&source(&workspace)).unwrap();
    let id = only_file(&library);

    library.trash_file(id).unwrap();
    let error = library.resolve_preview(id).unwrap_err();
    assert!(error.starts_with("Asset-Datei nicht im Katalog"), "{error}");
    let entry = library.trash_entries().unwrap()[0].id;
    library.restore_trash(entry).unwrap();
    library.resolve_preview(id).unwrap();

    library.trash_package(package.id).unwrap();
    assert!(library.resolve_preview(id).is_err());
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn zip_entries_are_materialized_into_the_cache_and_leave_the_archive_untouched() {
    let workspace = workspace("zip");
    let archive = workspace.join("Wald Paket ö.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    zip.start_file(
        format!("Ordner mit Leerzeichen/{NAME}"),
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(b"step").unwrap();
    zip.finish().unwrap();
    let archive_bytes = fs::read(&archive).unwrap();
    let mut library = Library::create(&workspace.join("library")).unwrap();
    library.reference_zip(&archive).unwrap();
    let id = only_file(&library);

    let cached = library.resolve_preview(id).unwrap();
    assert!(cached.starts_with(workspace.join("library/.cache/zip")));
    assert!(cached.ends_with(NAME));
    assert_eq!(fs::read(&cached).unwrap(), b"step");
    assert_eq!(fs::read(&archive).unwrap(), archive_bytes);

    fs::remove_file(&archive).unwrap();
    assert!(library.resolve_preview(id).is_err(), "offline ZIP");
    let _ = fs::remove_dir_all(workspace);
}
