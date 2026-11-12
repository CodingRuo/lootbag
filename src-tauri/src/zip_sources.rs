//! ZIP packs used in place: catalogued from the archive directory, entries extracted on demand
//! into `<library>/.cache/zip/<key>/`. The cache is disposable and never holds originals.
use super::imports::{
    available_space, checked_zip_path, create_safe_directories, open_zip, required_space,
};
use super::*;
use std::io::Read;
use zip::ZipArchive;

const IDENTITY_PREFIX: &str = "zip:";
/// The whole `.cache` is dropped on library open once it grows past this.
const CACHE_LIMIT: u64 = 4 << 30;

pub(super) fn is_zip(identity: &str) -> bool {
    identity.starts_with(IDENTITY_PREFIX)
}

/// File identity plus size and modification time: a moved archive keeps it, any rewrite changes it.
pub(super) fn archive_identity(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("ZIP-Datei nicht verfügbar. Bitte ZIP neu verknüpfen: {e}"))?;
    if !metadata.is_file() {
        return Err("ZIP-Quelle ist keine reguläre Datei".into());
    }
    Ok(format!(
        "{IDENTITY_PREFIX}{}|{}|{}",
        file_identity(path, &metadata)?,
        metadata.len(),
        modified_identity(&metadata)?
    ))
}

fn crc_text(crc: u32) -> String {
    format!("{crc:08x}")
}

fn cache_path(root: &Path, catalog_path: &str) -> Result<PathBuf> {
    let relative = Path::new(catalog_path);
    if !relative.starts_with("zip")
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Ungültiger Dateipfad im Katalog".into());
    }
    Ok(root.join(".cache").join(relative))
}

/// Writes one entry to `target` via a temporary sibling; size and CRC are checked while reading.
fn extract(root: &Path, entry: &mut dyn Read, size: u64, target: &Path) -> Result<()> {
    if available_space(root)? < required_space(size, 1)? {
        return Err("Nicht genug Speicherplatz zum Entpacken des ZIP-Eintrags".into());
    }
    let temporary = target.with_file_name(format!(".part-{}", unique_suffix()));
    let result = (|| {
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| format!("ZIP-Cache nicht schreibbar: {e}"))?;
        // Reading one byte past the declared size bounds a lying header and reaches the CRC check.
        let copied = io::copy(&mut Read::take(entry, size + 1), &mut output)
            .map_err(|e| format!("ZIP-Eintrag kann nicht entpackt werden: {e}"))?;
        if copied != size {
            return Err("ZIP-Eintrag hat nicht die erwartete Größe".into());
        }
        fs::rename(&temporary, target).map_err(|e| format!("ZIP-Cache nicht schreibbar: {e}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

impl Library {
    /// Catalogues a ZIP in place from its directory; no entry data is read or extracted.
    pub fn reference_zip(&mut self, source: &Path) -> Result<AssetPackage> {
        let source = source
            .canonical()
            .map_err(|e| format!("ZIP-Datei nicht verfügbar: {e}"))?;
        if source.starts_with(&self.root) {
            return Err("Bitte eine ZIP-Datei außerhalb der Bibliothek auswählen".into());
        }
        let identity = archive_identity(&source)?;
        let mut archive = open_zip(&source)?;
        let mut files = Vec::new();
        for index in 0..archive.len() {
            let entry = archive
                .by_index_raw(index)
                .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
            let path = checked_zip_path(&entry)?;
            if entry.encrypted() {
                return Err(format!(
                    "Verschlüsselte ZIP-Einträge werden nicht unterstützt: {}",
                    entry.name()
                ));
            }
            if entry.is_dir() {
                continue;
            }
            let Some(format) = assets::candidate_format(&path) else {
                continue;
            };
            files.push(ReferencedFile {
                relative: path,
                size: i64::try_from(entry.size())
                    .map_err(|_| "Datei ist zu groß für den Katalog")?,
                identity: crc_text(entry.crc32()),
                modified: "zip".into(),
                format: format.into(),
            });
        }
        if archive_identity(&source)? != identity {
            return Err("ZIP-Datei wurde während des Einbindens verändert".into());
        }
        let name = path_text(
            source
                .file_stem()
                .map(Path::new)
                .ok_or("ZIP-Datei hat keinen Namen")?,
        )?;
        self.save_reference(name, path_text(&source)?, identity, "zip", files)
    }

    /// Checks a referenced archive's identity; a changed archive loses its cache.
    fn check_package_zip(&self, location: &ReferencedLocation) -> Result<()> {
        if archive_identity(Path::new(&location.root)).ok().as_deref()
            != Some(location.root_identity.as_str())
        {
            if let Ok(cache) = cache_path(&self.root, &location.prefix) {
                let _ = fs::remove_dir_all(cache);
            }
            return Err(
                "ZIP-Datei wurde verschoben oder verändert. Bitte ZIP neu verknüpfen".into(),
            );
        }
        Ok(())
    }

    fn open_package_zip(&self, location: &ReferencedLocation) -> Result<ZipArchive<fs::File>> {
        self.check_package_zip(location)?;
        open_zip(Path::new(&location.root))
    }

    /// Archive index of a catalogued entry, checked against its recorded size and CRC.
    fn zip_entry_index(
        archive: &mut ZipArchive<fs::File>,
        relative: &str,
        location: &ReferencedLocation,
    ) -> Result<usize> {
        let entry_path = Path::new(relative)
            .strip_prefix(&location.prefix)
            .map_err(|_| "Ungültiger Dateipfad im Katalog")?;
        let index = archive
            .index_for_name(&path_text(entry_path)?)
            .ok_or("ZIP-Eintrag fehlt. Bitte ZIP neu verknüpfen")?;
        let entry = archive
            .by_index_raw(index)
            .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
        if checked_zip_path(&entry)? != entry_path
            || crc_text(entry.crc32()) != location.identity
            || i64::try_from(entry.size()).ok() != Some(location.size)
        {
            return Err("ZIP-Eintrag wurde verändert. Bitte ZIP neu verknüpfen".into());
        }
        Ok(index)
    }

    /// Cached copy of one catalogued entry, extracting it on first use.
    pub(super) fn locate_zip(
        &self,
        relative: &str,
        location: &ReferencedLocation,
    ) -> Result<PathBuf> {
        let target = cache_path(&self.root, relative)?;
        self.check_package_zip(location)?;
        let cached = Path::new(".cache").join(relative);
        create_safe_directories(&self.root, cached.parent().unwrap_or(Path::new("")))?;
        match fs::symlink_metadata(&target) {
            Ok(m) if m.is_file() && i64::try_from(m.len()).ok() == Some(location.size) => {
                return Ok(target)
            }
            Ok(m) if m.file_type().is_symlink() || m.is_dir() => {
                return Err("ZIP-Cache wurde durch eine Verknüpfung ersetzt".into())
            }
            _ => {}
        }
        let mut archive = open_zip(Path::new(&location.root))?;
        let index = Self::zip_entry_index(&mut archive, relative, location)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
        extract(&self.root, &mut entry, location.size as u64, &target)?;
        Ok(target)
    }

    /// Streams one catalogued entry to `read` without touching the cache.
    pub(super) fn read_zip<T>(
        &self,
        relative: &str,
        location: &ReferencedLocation,
        read: impl FnOnce(&mut dyn Read) -> T,
    ) -> Result<T> {
        let mut archive = self.open_package_zip(location)?;
        let index = Self::zip_entry_index(&mut archive, relative, location)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
        Ok(read(&mut entry))
    }

    /// Extracts a catalogued entry to a temporary file for readers that need a seekable file.
    /// The outer error means the archive is unavailable; the inner one is a read failure.
    pub(super) fn with_zip_file<T>(
        &self,
        relative: &str,
        location: &ReferencedLocation,
        read: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<Result<T>> {
        create_safe_directories(&self.root, Path::new(".cache"))?;
        let target = self
            .root
            .join(".cache")
            .join(format!(".scan-{}", unique_suffix()));
        let size = location.size as u64;
        let extracted = self.read_zip(relative, location, |entry| {
            extract(&self.root, entry, size, &target)
        })?;
        let result = extracted.and_then(|()| read(&target));
        let _ = fs::remove_file(&target);
        Ok(result)
    }

    /// Extracts the archive entries a model preview may need (`wanted` by archive path) into the cache.
    pub(super) fn extract_zip_resources(
        &self,
        location: &ReferencedLocation,
        wanted: impl Fn(&Path) -> bool,
    ) -> Result<()> {
        let mut archive = self.open_package_zip(location)?;
        let cache = Path::new(".cache").join(&location.prefix);
        for index in 0..archive.len() {
            if !archive
                .name_for_index(index)
                .is_some_and(|name| wanted(Path::new(name)))
            {
                continue;
            }
            let mut entry = archive
                .by_index(index)
                .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
            let path = checked_zip_path(&entry)?;
            if entry.is_dir() {
                continue;
            }
            let relative = cache.join(&path);
            create_safe_directories(&self.root, relative.parent().unwrap_or(Path::new("")))?;
            let target = self.root.join(&relative);
            if fs::symlink_metadata(&target).is_ok_and(|m| m.is_file() && m.len() == entry.size()) {
                continue;
            }
            let size = entry.size();
            // A broken companion is reported by the preview as missing, like in a folder.
            let _ = extract(&self.root, &mut entry, size, &target);
        }
        Ok(())
    }

    /// Drops the whole disposable cache when it outgrew its budget.
    pub(super) fn trim_zip_cache(&self) {
        let cache = self.root.join(".cache");
        if fs::symlink_metadata(&cache).is_ok_and(|m| m.is_dir()) && dir_size(&cache) > CACHE_LIMIT
        {
            let _ = fs::remove_dir_all(&cache);
        }
    }

    pub(super) fn remove_zip_cache(&self, prefix: &str) {
        if let Ok(cache) = cache_path(&self.root, prefix) {
            let _ = fs::remove_dir_all(cache);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
    };

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "lootbag-zip-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonical().unwrap())
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            if name.ends_with('/') {
                zip.add_directory(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
            } else {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(bytes).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    fn files_below(path: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                if entry.file_type().unwrap().is_dir() {
                    found.extend(files_below(&entry.path()));
                } else {
                    found.push(entry.path());
                }
            }
        }
        found
    }

    fn file_id(library: &Library, name: &str) -> i64 {
        library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .iter()
            .flat_map(|asset| asset.files.iter())
            .find(|file| file.original_filename == name)
            .unwrap_or_else(|| panic!("{name} not catalogued"))
            .id
    }

    #[test]
    fn a_zip_is_catalogued_from_its_directory_and_previews_extract_one_cached_entry() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("Forest Pack.zip");
        write_zip(
            &archive,
            &[
                ("Sounds/", b""),
                ("Sounds/birds.wav", b"bird audio"),
                ("Sounds/birds.mp3", b"bird mp3"),
                ("Textures/leaf.png", b"leaf png"),
                ("readme.txt", b"companion"),
            ],
        );
        let archive_bytes = fs::read(&archive).unwrap();
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();

        let package = library.reference_zip(&archive).unwrap();

        assert_eq!(package.name, "Forest Pack");
        assert!(package.zip);
        assert_eq!(package.source_kind, "external");
        assert_eq!(package.asset_count, 2, "variants and groups as in folders");
        let page = library.query(CatalogQuery::default()).unwrap();
        assert!(page
            .items
            .iter()
            .all(|a| a.files.iter().all(|f| f.available)));
        assert_eq!(
            files_below(&root),
            vec![root.join("catalog.sqlite3")],
            "nothing is extracted while cataloguing or listing"
        );

        let wav = file_id(&library, "birds.wav");
        let preview = library.resolve_preview(wav).unwrap();
        assert!(preview.starts_with(root.join(".cache/zip")));
        assert_eq!(fs::read(&preview).unwrap(), b"bird audio");
        assert_eq!(files_below(&root.join(".cache")), vec![preview.clone()]);
        let modified = fs::metadata(&preview).unwrap().modified().unwrap();
        assert_eq!(library.resolve_preview(wav).unwrap(), preview);
        assert_eq!(
            fs::metadata(&preview).unwrap().modified().unwrap(),
            modified
        );

        let export = workspace.0.join("out.wav");
        library.export(wav, Some(&export), false).unwrap();
        assert_eq!(fs::read(&export).unwrap(), b"bird audio");
        assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
    }

    #[test]
    fn unsafe_and_encrypted_entries_reject_the_whole_zip() {
        let workspace = Workspace::new();
        let mut library = Library::create(&workspace.0.join("library")).unwrap();
        for (name, entry) in [
            ("traversal", "../escape.wav"),
            ("parent", "a/../escape.wav"),
            ("absolute", "/escape.wav"),
            ("backslash", "..\\escape.wav"),
            ("drive", "C:/escape.wav"),
            ("drive-relative", "C:escape.wav"),
            ("unc", "\\\\server\\share\\escape.wav"),
            ("stream", "escape.wav:hidden"),
        ] {
            let archive = workspace.0.join(format!("{name}.zip"));
            write_zip(&archive, &[(entry, b"x"), ("ok.wav", b"ok")]);
            assert!(library.reference_zip(&archive).is_err(), "accepted {entry}");
        }
        let linked = workspace.0.join("link.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&linked).unwrap());
        zip.add_symlink(
            "outside.wav",
            "../x",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        assert!(library.reference_zip(&linked).is_err());
        // Set the encryption flag in the local and central headers of a plain archive.
        let encrypted = workspace.0.join("encrypted.zip");
        write_zip(&encrypted, &[("secret.wav", b"secret")]);
        let mut bytes = fs::read(&encrypted).unwrap();
        for (signature, flags) in [(b"PK\x03\x04", 6), (b"PK\x01\x02", 8)] {
            let at = bytes.windows(4).position(|w| w == signature).unwrap();
            bytes[at + flags] |= 1;
        }
        fs::write(&encrypted, bytes).unwrap();
        let error = library.reference_zip(&encrypted).unwrap_err();
        assert!(error.contains("Verschlüsselt"), "{error}");
        assert!(library.packages().unwrap().is_empty());
        assert!(!workspace.0.join("escape.wav").exists());
    }

    #[test]
    fn a_changed_archive_drops_its_cache_and_only_the_moved_original_relinks() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("pack.zip");
        write_zip(&archive, &[("hit.wav", b"hit")]);
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.reference_zip(&archive).unwrap();
        let hit = file_id(&library, "hit.wav");
        let cached = library.resolve_preview(hit).unwrap();

        let moved = workspace.0.join("moved.zip");
        fs::rename(&archive, &moved).unwrap();
        assert!(!library.packages().unwrap()[0].available);
        assert!(library
            .resolve_preview(hit)
            .unwrap_err()
            .contains("neu verknüpfen"));
        let copy = workspace.0.join("copy.zip");
        fs::copy(&moved, &copy).unwrap();
        assert!(library.relink_source(package.id, &copy).is_err());
        assert!(library.relink_source(package.id, &moved).unwrap().available);
        assert_eq!(
            fs::read(library.resolve_preview(hit).unwrap()).unwrap(),
            b"hit"
        );

        write_zip(&moved, &[("hit.wav", b"changed hit")]);
        assert!(library.resolve_preview(hit).is_err());
        assert!(!cached.exists(), "cache of a changed archive is dropped");
        assert!(!library.query(CatalogQuery::default()).unwrap().items[0].files[0].available);
        assert!(library.relink_source(package.id, &moved).is_err());
    }

    fn glb(json: &str) -> Vec<u8> {
        let total = 20 + json.len() as u32;
        let mut bytes = b"glTF".to_vec();
        for value in [2, total, json.len() as u32, 0x4e4f534a] {
            bytes.extend(u32::to_le_bytes(value));
        }
        bytes.extend(json.as_bytes());
        bytes
    }

    #[test]
    fn model_previews_and_exports_take_companions_from_the_same_archive() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("trees.zip");
        let gltf = br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"tree.bin"}],"images":[{"uri":"tex/leaf.png"}]}"#;
        write_zip(
            &archive,
            &[
                ("Tree/tree.gltf", gltf),
                ("Tree/tree.bin", b"mesh bytes"),
                ("Tree/tex/leaf.png", b"leaf png"),
                ("Sounds/wind.wav", b"wind"),
            ],
        );
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&archive).unwrap();
        let model = file_id(&library, "tree.gltf");

        let snapshot = library.model_resources(model).unwrap();

        assert!(snapshot.errors.is_empty(), "{:?}", snapshot.errors);
        let mut paths: Vec<_> = snapshot
            .resources
            .iter()
            .map(|r| r.relative_path.as_str())
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            ["Tree/tex/leaf.png", "Tree/tree.bin", "Tree/tree.gltf"]
        );
        assert!(snapshot
            .resources
            .iter()
            .all(|r| Path::new(&r.path).starts_with(root.join(".cache/zip"))));
        assert!(
            !files_below(&root.join(".cache"))
                .iter()
                .any(|p| p.ends_with("wind.wav")),
            "sounds are not model companions"
        );
        let destination = workspace.0.join("export/tree-model");
        fs::create_dir(workspace.0.join("export")).unwrap();
        library
            .export_model(&snapshot, &[], Some(&destination))
            .unwrap();
        assert_eq!(fs::read(destination.join("Tree/tree.gltf")).unwrap(), gltf);
        assert_eq!(
            fs::read(destination.join("Tree/tree.bin")).unwrap(),
            b"mesh bytes"
        );
        assert_eq!(
            fs::read(destination.join("Tree/tex/leaf.png")).unwrap(),
            b"leaf png"
        );
    }

    #[test]
    fn hashes_and_model_traits_stream_zip_entries_without_caching_them() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("pack.zip");
        let mesh = glb(r#"{"asset":{"version":"2.0"},"meshes":[{"primitives":[{}]}]}"#);
        write_zip(
            &archive,
            &[
                ("A/step.wav", b"same bytes"),
                ("B/stomp.wav", b"same bytes"),
                ("C/other.wav", b"diff bytes"),
                ("rock.glb", &mesh),
            ],
        );
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&archive).unwrap();
        let cancel = AtomicBool::new(false);

        let (hashed, _) = library.hash_pending_files(&mut 0, 100, &cancel).unwrap();
        let (inspected, _) = library
            .inspect_pending_models(&mut 0, 100, &cancel)
            .unwrap();

        assert_eq!((hashed, inspected), (3, 1));
        let items = library.query(CatalogQuery::default()).unwrap().items;
        let duplicates = |name: &str| items.iter().find(|a| a.name == name).unwrap().duplicates;
        assert_eq!(
            (duplicates("step"), duplicates("stomp"), duplicates("other")),
            (1, 1, 0)
        );
        let rock = items.iter().find(|a| a.name == "rock").unwrap();
        assert_eq!(rock.files[0].model_traits.unwrap().has_mesh, Some(true));
        assert!(
            files_below(&root.join(".cache")).is_empty(),
            "no cached or leftover copies"
        );
        // Done files are not picked up again.
        assert_eq!(
            library.hash_pending_files(&mut 0, 100, &cancel).unwrap(),
            (0, 0)
        );
        assert_eq!(
            library
                .inspect_pending_models(&mut 0, 100, &cancel)
                .unwrap(),
            (0, 0)
        );
    }

    #[test]
    fn emptying_the_trash_removes_zip_rows_and_cache_but_never_the_archive() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("pack.zip");
        write_zip(&archive, &[("hit.wav", b"hit"), ("miss.wav", b"miss")]);
        let archive_bytes = fs::read(&archive).unwrap();
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.reference_zip(&archive).unwrap();
        let cached = library
            .resolve_preview(file_id(&library, "hit.wav"))
            .unwrap();
        library.trash_package(package.id).unwrap();
        assert!(cached.exists(), "a trashed package can still be restored");

        let emptied = library.empty_trash().unwrap();

        assert_eq!(emptied.packages, 1);
        assert!(emptied.disk_errors.is_empty(), "{:?}", emptied.disk_errors);
        assert!(!root.join(".cache").join(&package.relative_path).exists());
        assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
        for table in [
            "packages",
            "asset_files",
            "external_sources",
            "external_files",
        ] {
            let count: i64 = library
                .connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }

    #[test]
    fn an_oversized_cache_is_dropped_when_the_library_opens() {
        let workspace = Workspace::new();
        let root = workspace.0.join("library");
        drop(Library::create(&root).unwrap());
        let big = root.join(".cache/zip/old/big.bin");
        fs::create_dir_all(big.parent().unwrap()).unwrap();
        fs::File::create(&big)
            .unwrap()
            .set_len(CACHE_LIMIT + 1)
            .unwrap();
        Library::open(&root).unwrap();
        assert!(!root.join(".cache").exists());
    }
}
