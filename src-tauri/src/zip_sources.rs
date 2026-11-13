//! ZIP packs used in place: catalogued from the archive directory, entries extracted on demand
//! into `<library>/.cache/zip/<key>/`. The cache is disposable and never holds originals.
//! Unity packages share this machinery; the entry points below dispatch on the archive kind
//! (see `unity_packages`) and cache them under `<library>/.cache/unity/<key>/`.
use super::imports::{
    available_space, checked_zip_path, create_safe_directories, open_zip, required_space,
};
use super::unity_packages::{self as unity, is_unity, is_unity_path};
use super::*;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
};
use zip::ZipArchive;

const IDENTITY_PREFIX: &str = "zip:";
/// The whole `.cache` is dropped on library open once it grows past this.
const CACHE_LIMIT: u64 = 4 << 30;

/// Whether a source identity belongs to an archive used in place (ZIP or Unity package).
pub(super) fn is_zip(identity: &str) -> bool {
    identity.starts_with(IDENTITY_PREFIX) || is_unity(identity)
}

/// File identity plus size and modification time: a moved archive keeps it, any rewrite changes it.
pub(super) fn archive_identity(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("Archiv nicht verfügbar. Bitte neu verknüpfen: {e}"))?;
    if !metadata.is_file() {
        return Err("Archivquelle ist keine reguläre Datei".into());
    }
    Ok(format!(
        "{}{}|{}|{}",
        if is_unity_path(path) {
            unity::IDENTITY_PREFIX
        } else {
            IDENTITY_PREFIX
        },
        file_identity(path, &metadata)?,
        metadata.len(),
        modified_identity(&metadata)?
    ))
}

pub(super) fn crc_text(crc: u32) -> String {
    format!("{crc:08x}")
}

fn cache_path(root: &Path, catalog_path: &str) -> Result<PathBuf> {
    let relative = Path::new(catalog_path);
    if !(relative.starts_with("zip") || relative.starts_with("unity"))
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

/// Cache files whose CRC was checked (or written) this session, by canonical path, with the size,
/// modification time and CRC they had then; a change to either is checked again.
// shortcut: bytes changed inside the cache with size and modification time restored are not noticed
// until the entry is evicted (cache deletion, library switch); the cache is the app's own folder.
type Verified = HashMap<PathBuf, (u64, Option<std::time::SystemTime>, String)>;
static VERIFIED: std::sync::Mutex<Option<Verified>> = std::sync::Mutex::new(None);

#[cfg(test)]
thread_local!(static CRC_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });

fn verified_key(target: &Path) -> PathBuf {
    target.canonical().unwrap_or_else(|_| target.to_owned())
}

fn remember_verified(target: &Path, crc: &str) {
    if let (Ok(m), Ok(mut verified)) = (fs::symlink_metadata(target), VERIFIED.lock()) {
        verified.get_or_insert_with(HashMap::new).insert(
            verified_key(target),
            (m.len(), m.modified().ok(), crc.to_owned()),
        );
    }
}

/// Forgets verified cache files below `dir`, or all of them.
fn forget_verified(dir: Option<&Path>) {
    if let Ok(mut verified) = VERIFIED.lock() {
        if let Some(verified) = verified.as_mut() {
            verified.retain(|path, _| dir.is_some_and(|dir| !path.starts_with(dir)));
        }
    }
}

/// On library activation: entries of another library or an earlier session are not trusted.
pub(crate) fn forget_verified_zip_cache() {
    forget_verified(None);
}

/// Whether `target` already holds the entry with this size and CRC; a link or folder there is refused.
fn cached(target: &Path, size: u64, crc: &str) -> Result<bool> {
    match fs::symlink_metadata(target) {
        Ok(m) if m.file_type().is_symlink() || m.is_dir() => {
            Err("ZIP-Cache wurde durch eine Verknüpfung ersetzt".into())
        }
        Ok(m) if m.len() == size => {
            let stamp = (m.len(), m.modified().ok(), crc.to_owned());
            if VERIFIED.lock().is_ok_and(|verified| {
                verified.as_ref().and_then(|v| v.get(&verified_key(target))) == Some(&stamp)
            }) {
                return Ok(true);
            }
            #[cfg(test)]
            CRC_READS.with(|reads| reads.set(reads.get() + 1));
            let Ok(mut file) = fs::File::open(target) else {
                return Ok(false);
            };
            let mut hasher = crc32fast::Hasher::new();
            let mut buffer = vec![0; 1 << 16];
            loop {
                match file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => hasher.update(&buffer[..read]),
                    Err(_) => return Ok(false),
                }
            }
            let valid = crc_text(hasher.finalize()) == crc;
            if valid {
                remember_verified(target, crc);
            }
            Ok(valid)
        }
        _ => Ok(false),
    }
}

/// Removes `target` below `<library>/.cache`, but never through a link that could lead outside.
fn remove_cache_dir(root: &Path, target: &Path) {
    let Ok(inside) = target.strip_prefix(root) else {
        return;
    };
    let mut path = root.to_owned();
    for part in inside.components() {
        path.push(part);
        if !fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_symlink()) {
            return;
        }
    }
    let (Ok(cache), Ok(canonical)) = (root.join(".cache").canonical(), target.canonical()) else {
        return;
    };
    let Some(parent) = canonical.parent().filter(|_| canonical.starts_with(&cache)) else {
        return;
    };
    // Renaming moves the checked directory itself (the last component is never followed) to a name
    // nothing else uses; only that renamed directory, still in the checked parent, is deleted.
    // shortcut: a parent swapped for a link between the checks and the rename can still redirect it;
    // single-user local app, whoever can swap folders in the library already owns its files.
    forget_verified(Some(&canonical));
    let moved = parent.join(format!(".remove-{}", unique_suffix()));
    if fs::rename(&canonical, &moved).is_ok()
        && moved
            .canonical()
            .is_ok_and(|path| path.parent() == Some(parent))
        && fs::symlink_metadata(&moved).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        let _ = fs::remove_dir_all(&moved);
    }
}

/// Archive paths seen so far, lowercased, to find paths that would share one cache path on
/// case-insensitive disks, as would a file named like another entry's folder (`a.wav` and
/// `A.WAV/x.wav`).
// shortcut: lowercase only, so NFC/NFD spellings of one name (e.g. "ä") still pass; normalize once a Unicode crate is in the tree.
#[derive(Default)]
pub(super) struct PathSet {
    names: HashSet<String>,
    dirs: HashSet<String>,
}

impl PathSet {
    /// Adds a file or folder path; false when it collides with one added before.
    pub(super) fn insert(&mut self, path: &str, dir: bool) -> bool {
        let lower = path.to_lowercase();
        let parents: Vec<_> = lower
            .match_indices('/')
            .map(|(at, _)| lower[..at].to_owned())
            .collect();
        let collides = parents.iter().any(|parent| self.names.contains(parent))
            || if dir {
                self.names.contains(&lower)
            } else {
                self.dirs.contains(&lower) || !self.names.insert(lower.clone())
            };
        if !collides {
            self.dirs.extend(parents);
            if dir {
                self.dirs.insert(lower);
            }
        }
        !collides
    }
}

/// Validates an archive's whole directory with the ZIP rules and lists its candidate entries;
/// no entry data is read. Fails when the archive changes while it is read.
pub(super) fn read_directory(source: &Path) -> Result<(String, Vec<ReferencedFile>)> {
    if is_unity_path(source) {
        return unity::read_directory(source);
    }
    let identity = archive_identity(source)?;
    let mut archive = open_zip(source)?;
    let mut files = Vec::new();
    let mut paths = PathSet::default();
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
        if !paths.insert(&path_text(&path)?, entry.is_dir()) {
            return Err(format!(
                "ZIP enthält Pfade, die sich nur in Groß-/Kleinschreibung unterscheiden oder Datei und Ordner zugleich sind: {}",
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
            size: i64::try_from(entry.size()).map_err(|_| "Datei ist zu groß für den Katalog")?,
            identity: crc_text(entry.crc32()),
            modified: "zip".into(),
            format: format.into(),
        });
    }
    if archive_identity(source)? != identity {
        return Err("ZIP-Datei wurde während des Lesens verändert".into());
    }
    Ok((identity, files))
}

/// Archives opened during one backfill batch, by source identity, so each directory is parsed once
/// and a Unity package is read in one forward pass.
pub(super) type OpenArchives = HashMap<String, OpenArchive>;

pub(super) enum OpenArchive {
    Zip(ZipArchive<fs::File>),
    Unity(Box<unity::Cursor>),
}

/// Serializes Unity cache extraction: a concurrent request for the same payload waits and then
/// finds the verified copy instead of decompressing the package again.
static UNITY_EXTRACTION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The catalogued payload of a Unity package file: GUID in `modified`, CRC in `identity`.
fn unity_payload(relative: &str, location: &ReferencedLocation) -> Result<unity::Payload> {
    Ok(unity::Payload {
        path: Path::new(relative)
            .strip_prefix(&location.prefix)
            .map_err(|_| "Ungültiger Dateipfad im Katalog")?
            .to_owned(),
        guid: location.modified.clone(),
        size: location.size as u64,
        crc: location.identity.clone(),
    })
}

#[cfg(test)]
thread_local!(static ARCHIVE_OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });

/// Fails reads once `cancel` is set, so a long entry stops between chunks.
pub(super) struct Cancellable<'a, R>(pub &'a AtomicBool, pub R);
impl<R: Read> Read for Cancellable<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.0.load(Ordering::Relaxed) {
            return Err(io::Error::other("abgebrochen"));
        }
        self.1.read(buffer)
    }
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
    /// Catalogues a ZIP in place from its directory, or a `.unitypackage` from one full scan;
    /// nothing is extracted.
    pub fn reference_zip(&mut self, source: &Path) -> Result<AssetPackage> {
        let source = source
            .canonical()
            .map_err(|e| format!("Archiv nicht verfügbar: {e}"))?;
        if source.starts_with(&self.root) {
            return Err("Bitte ein Archiv außerhalb der Bibliothek auswählen".into());
        }
        let (identity, files) = read_directory(&source)?;
        let name = path_text(
            source
                .file_stem()
                .map(Path::new)
                .ok_or("Archiv hat keinen Namen")?,
        )?;
        let kind = if is_unity_path(&source) {
            "unity"
        } else {
            "zip"
        };
        self.save_reference(name, path_text(&source)?, identity, kind, files)
    }

    /// Checks a referenced archive's identity; a changed archive loses its cache.
    fn check_package_zip(&self, location: &ReferencedLocation) -> Result<()> {
        if archive_identity(Path::new(&location.root)).ok().as_deref()
            != Some(location.root_identity.as_str())
        {
            if let Ok(cache) = cache_path(&self.root, &location.prefix) {
                remove_cache_dir(&self.root, &cache);
            }
            return Err(if is_unity(&location.root_identity) {
                "Unity-Paket wurde verschoben oder verändert. Bitte neu verknüpfen".into()
            } else {
                "ZIP-Datei wurde verschoben oder verändert. Bitte ZIP neu verknüpfen".into()
            });
        }
        Ok(())
    }

    fn open_package_zip(&self, location: &ReferencedLocation) -> Result<ZipArchive<fs::File>> {
        self.check_package_zip(location)?;
        #[cfg(test)]
        ARCHIVE_OPENS.with(|opens| opens.set(opens.get() + 1));
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
        let cache = Path::new(".cache").join(relative);
        create_safe_directories(&self.root, cache.parent().unwrap_or(Path::new("")))?;
        if cached(&target, location.size as u64, &location.identity)? {
            return Ok(target);
        }
        if is_unity(&location.root_identity) {
            self.extract_unity(location, &[&unity_payload(relative, location)?])?
                .pop()
                .expect("one payload")?;
            return Ok(target);
        }
        let mut archive = open_zip(Path::new(&location.root))?;
        let index = Self::zip_entry_index(&mut archive, relative, location)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
        extract(&self.root, &mut entry, location.size as u64, &target)?;
        remember_verified(&target, &location.identity);
        Ok(target)
    }

    /// Streams one catalogued entry to `read` without touching the cache, reusing `archives`.
    pub(super) fn read_zip<T>(
        &self,
        archives: &mut OpenArchives,
        relative: &str,
        location: &ReferencedLocation,
        cancel: &AtomicBool,
        read: impl FnOnce(&mut dyn Read) -> T,
    ) -> Result<T> {
        let archive = match archives.entry(location.root_identity.clone()) {
            Entry::Occupied(open) => open.into_mut(),
            Entry::Vacant(slot) if is_unity(&location.root_identity) => {
                self.check_package_zip(location)?;
                slot.insert(OpenArchive::Unity(Box::new(unity::Cursor::new(Path::new(
                    &location.root,
                )))))
            }
            Entry::Vacant(slot) => slot.insert(OpenArchive::Zip(self.open_package_zip(location)?)),
        };
        let archive = match archive {
            OpenArchive::Zip(archive) => archive,
            OpenArchive::Unity(cursor) => {
                return cursor.read(&unity_payload(relative, location)?, cancel, read);
            }
        };
        let index = Self::zip_entry_index(archive, relative, location)?;
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("ZIP-Eintrag nicht lesbar: {e}"))?;
        Ok(read(&mut Cancellable(cancel, entry)))
    }

    /// Extracts a catalogued entry to a temporary file for readers that need a seekable file.
    /// The outer error means the archive is unavailable; the inner one is a read failure.
    pub(super) fn with_zip_file<T>(
        &self,
        archives: &mut OpenArchives,
        relative: &str,
        location: &ReferencedLocation,
        cancel: &AtomicBool,
        read: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<Result<T>> {
        create_safe_directories(&self.root, Path::new(".cache"))?;
        let target = self
            .root
            .join(".cache")
            .join(format!(".scan-{}", unique_suffix()));
        let size = location.size as u64;
        let extracted = self.read_zip(archives, relative, location, cancel, |entry| {
            extract(&self.root, entry, size, &target)
        })?;
        let result = extracted.and_then(|()| read(&target));
        let _ = fs::remove_file(&target);
        Ok(result)
    }

    /// Extracts the archive entries a model preview needs (`select`ed from all archive paths) into
    /// the cache; returns their archive paths.
    pub(super) fn extract_zip_resources(
        &self,
        location: &ReferencedLocation,
        select: impl FnOnce(&[&str]) -> Vec<String>,
    ) -> Result<Vec<PathBuf>> {
        if is_unity(&location.root_identity) {
            self.check_package_zip(location)?;
            let payloads = unity::payloads(Path::new(&location.root), &location.root_identity)?;
            let names: Vec<_> = payloads.iter().filter_map(|p| p.path.to_str()).collect();
            let wanted: HashSet<String> = select(&names).into_iter().collect();
            let wanted: Vec<_> = payloads
                .iter()
                .filter(|p| p.path.to_str().is_some_and(|name| wanted.contains(name)))
                .collect();
            // A broken companion is reported by the preview as missing, like in a folder.
            self.extract_unity(location, &wanted)?;
            return Ok(wanted.into_iter().map(|p| p.path.clone()).collect());
        }
        let mut archive = self.open_package_zip(location)?;
        let wanted: HashSet<String> = select(&archive.file_names().collect::<Vec<_>>())
            .into_iter()
            .collect();
        let cache = Path::new(".cache").join(&location.prefix);
        let mut found = Vec::new();
        for index in 0..archive.len() {
            if !archive
                .name_for_index(index)
                .is_some_and(|name| wanted.contains(name))
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
            let size = entry.size();
            // A broken companion is reported by the preview as missing, like in a folder.
            let crc = crc_text(entry.crc32());
            if !cached(&target, size, &crc)?
                && extract(&self.root, &mut entry, size, &target).is_ok()
            {
                remember_verified(&target, &crc);
            }
            found.push(path);
        }
        Ok(found)
    }

    /// Extracts Unity payloads (in archive order) into the package cache in one pass, skipping
    /// verified copies; returns each payload's result.
    fn extract_unity(
        &self,
        location: &ReferencedLocation,
        payloads: &[&unity::Payload],
    ) -> Result<Vec<Result<()>>> {
        let _serial = UNITY_EXTRACTION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.check_package_zip(location)?;
        let cache = Path::new(".cache").join(&location.prefix);
        let mut cursor = unity::Cursor::new(Path::new(&location.root));
        let cancel = AtomicBool::new(false);
        Ok(payloads
            .iter()
            .map(|payload| {
                let relative = cache.join(&payload.path);
                create_safe_directories(&self.root, relative.parent().unwrap_or(Path::new("")))?;
                let target = self.root.join(&relative);
                if !cached(&target, payload.size, &payload.crc)? {
                    cursor.read(payload, &cancel, |entry| {
                        extract(&self.root, entry, payload.size, &target)
                    })??;
                    remember_verified(&target, &payload.crc);
                }
                Ok(())
            })
            .collect())
    }

    /// Drops the whole disposable cache when it outgrew its budget.
    pub(super) fn trim_zip_cache(&self) {
        let cache = self.root.join(".cache");
        if fs::symlink_metadata(&cache).is_ok_and(|m| m.is_dir()) && dir_size(&cache) > CACHE_LIMIT
        {
            remove_cache_dir(&self.root, &cache);
        }
    }

    pub(super) fn remove_zip_cache(&self, prefix: &str) {
        if let Ok(cache) = cache_path(&self.root, prefix) {
            remove_cache_dir(&self.root, &cache);
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
    fn an_obj_preview_extracts_only_the_resources_it_names() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("props.zip");
        write_zip(
            &archive,
            &[
                ("Props/crate.obj", b"mtllib crate.mtl\nv 0 0 0\n"),
                (
                    "Props/crate.mtl",
                    b"newmtl wood\nmap_Kd Textures/crate_wood.png\n",
                ),
                ("Props/Textures/crate_wood.png", b"wood"),
                ("Props/Textures/barrel.png", b"barrel"),
                ("Props/barrel.mtl", b"map_Kd barrel.png\n"),
                ("Other/grass.png", b"grass"),
                ("Other/data.bin", b"bin"),
            ],
        );
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&archive).unwrap();

        library
            .model_resources(file_id(&library, "crate.obj"))
            .unwrap();

        let cache = root.join(".cache");
        let mut extracted: Vec<_> = files_below(&cache)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        extracted.sort();
        assert_eq!(extracted, ["crate.mtl", "crate.obj", "crate_wood.png"]);
    }

    #[test]
    fn a_gltf_buffer_with_any_extension_is_taken_from_the_archive() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("model.zip");
        let gltf = br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"mesh.dat"}]}"#;
        write_zip(&archive, &[("model.gltf", gltf), ("mesh.dat", b"mesh")]);
        let mut library = Library::create(&workspace.0.join("library")).unwrap();
        library.reference_zip(&archive).unwrap();

        let snapshot = library
            .model_resources(file_id(&library, "model.gltf"))
            .unwrap();

        assert!(snapshot.errors.is_empty(), "{:?}", snapshot.errors);
        assert!(snapshot
            .resources
            .iter()
            .any(|r| r.relative_path == "mesh.dat"));
    }

    #[test]
    fn a_texture_named_with_its_folder_skips_same_named_files_elsewhere() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("props.zip");
        write_zip(
            &archive,
            &[
                ("Props/crate.obj", b"mtllib crate.mtl\n"),
                ("Props/crate.mtl", b"map_Kd Textures/albedo.png\n"),
                ("Props/Textures/albedo.png", b"crate"),
                ("Other/albedo.png", b"other"),
            ],
        );
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&archive).unwrap();

        library
            .model_resources(file_id(&library, "crate.obj"))
            .unwrap();

        let cache = files_below(&root.join(".cache"));
        assert!(cache
            .iter()
            .any(|p| p.ends_with("Props/Textures/albedo.png")));
        assert!(
            !cache.iter().any(|p| p.ends_with("Other/albedo.png")),
            "{cache:?}"
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
                ("stone.glb", &mesh),
            ],
        );
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&archive).unwrap();
        let cancel = AtomicBool::new(false);
        let opens = || ARCHIVE_OPENS.with(|opens| opens.replace(0));
        opens();

        let (hashed, ..) = library.hash_pending_files(&mut 0, 100, &cancel).unwrap();
        assert_eq!(opens(), 1, "one archive directory parse per batch");
        let (inspected, _) = library
            .inspect_pending_models(&mut 0, 100, &cancel)
            .unwrap();
        assert_eq!(opens(), 1);

        assert_eq!((hashed, inspected), (5, 2));
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
            (0, 0, 0)
        );
        assert_eq!(
            library
                .inspect_pending_models(&mut 0, 100, &cancel)
                .unwrap(),
            (0, 0)
        );
    }

    #[test]
    fn a_rewritten_archive_drops_its_entry_hashes_and_duplicates() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("pack.zip");
        write_zip(
            &archive,
            &[
                ("A/step.wav", b"same bytes"),
                ("B/stomp.wav", b"same bytes"),
            ],
        );
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&archive).unwrap();
        let cancel = AtomicBool::new(false);
        library.hash_pending_files(&mut 0, 100, &cancel).unwrap();
        let duplicates = |library: &Library| {
            library
                .query(CatalogQuery::default())
                .unwrap()
                .items
                .iter()
                .map(|a| a.duplicates)
                .sum::<i64>()
        };
        assert_eq!(duplicates(&library), 2);

        write_zip(
            &archive,
            &[
                ("A/step.wav", b"other bytes!"),
                ("B/stomp.wav", b"same bytes"),
            ],
        );
        library.hash_pending_files(&mut 0, 100, &cancel).unwrap();

        assert_eq!(
            duplicates(&library),
            0,
            "hashes of the old archive are dropped"
        );
        let hashes: i64 = library
            .connection
            .query_row("SELECT count(*) FROM file_hashes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(hashes, 0);
    }

    /// Tests that count CRC reads, since they share the process-wide verified map.
    static VERIFIED_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn deleting_the_cache_or_switching_libraries_forgets_verified_entries() {
        let _serial = VERIFIED_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let workspace = Workspace::new();
        let archive = workspace.0.join("pack.zip");
        write_zip(&archive, &[("hit.wav", b"hit audio")]);
        let mut library = Library::create(&workspace.0.join("library")).unwrap();
        let package = library.reference_zip(&archive).unwrap();
        let hit = file_id(&library, "hit.wav");
        let reads = || CRC_READS.with(|reads| reads.replace(0));
        let cached = library.resolve_preview(hit).unwrap();
        let stamp = fs::metadata(&cached).unwrap().modified().unwrap();
        reads();

        forget_verified_zip_cache();
        library.resolve_preview(hit).unwrap();
        assert_eq!(reads(), 1, "a library switch checks the cache again");

        // Deleted with its cache directory, then put back with the same size and time but other bytes.
        library.remove_zip_cache(&package.relative_path);
        assert!(!cached.exists());
        fs::create_dir_all(cached.parent().unwrap()).unwrap();
        fs::write(&cached, b"HIT AUDIO").unwrap();
        fs::File::options()
            .write(true)
            .open(&cached)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        assert_eq!(
            fs::read(library.resolve_preview(hit).unwrap()).unwrap(),
            b"hit audio"
        );
        assert_eq!(reads(), 1, "the deleted entry was forgotten");
    }

    #[test]
    fn a_verified_cache_hit_is_not_read_again_until_it_changes() {
        let _serial = VERIFIED_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let workspace = Workspace::new();
        let archive = workspace.0.join("pack.zip");
        write_zip(&archive, &[("hit.wav", b"hit audio")]);
        let mut library = Library::create(&workspace.0.join("library")).unwrap();
        library.reference_zip(&archive).unwrap();
        let hit = file_id(&library, "hit.wav");
        let reads = || CRC_READS.with(|reads| reads.replace(0));
        let cached = library.resolve_preview(hit).unwrap();
        reads();

        library.resolve_preview(hit).unwrap();
        library.resolve_preview(hit).unwrap();
        assert_eq!(reads(), 0, "extracted entries count as verified");

        fs::write(&cached, b"HIT AUDIO").unwrap();
        assert_eq!(
            fs::read(library.resolve_preview(hit).unwrap()).unwrap(),
            b"hit audio"
        );
        assert_eq!(reads(), 1, "a changed file is checked again");
    }

    fn location(library: &Library, file_id: i64) -> (String, ReferencedLocation) {
        library
            .connection
            .query_row(
                "SELECT f.relative_path, s.root, s.identity, p.relative_path, e.identity, e.modified, f.size_bytes FROM asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id JOIN external_sources s ON s.package_id = p.id JOIN external_files e ON e.file_id = f.id WHERE f.id = ?1",
                [file_id],
                |row| Ok((row.get(0)?, location_row(row, 1)?.unwrap())),
            )
            .unwrap()
    }

    #[test]
    fn a_cancelled_backfill_stops_inside_an_entry_and_leaves_nothing_behind() {
        let workspace = Workspace::new();
        let archive = workspace.0.join("pack.zip");
        write_zip(&archive, &[("big.fbx", &vec![7; 3 << 20])]);
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&archive).unwrap();
        let (relative, zip) = location(&library, file_id(&library, "big.fbx"));
        let cancel = AtomicBool::new(true);
        let mut archives = OpenArchives::new();

        let read = library
            .read_zip(&mut archives, &relative, &zip, &cancel, |entry| {
                entry.read_to_end(&mut Vec::new())
            })
            .unwrap();
        let extracted = library
            .with_zip_file(&mut archives, &relative, &zip, &cancel, |_| Ok(()))
            .unwrap();

        assert!(read.is_err() && extracted.is_err());
        assert!(files_below(&root.join(".cache")).is_empty());
        assert_eq!(
            library
                .inspect_pending_models(&mut 0, 100, &cancel)
                .unwrap(),
            (0, 1),
            "nothing recorded, still pending"
        );
    }

    #[test]
    fn case_colliding_entries_reject_the_zip_and_a_tampered_cache_is_extracted_again() {
        let workspace = Workspace::new();
        let mut library = Library::create(&workspace.0.join("library")).unwrap();
        for entries in [
            [("Hit.wav", b"one"), ("hit.WAV", b"two")],
            [("Tex/leaf.png", b"one"), ("tex/Leaf.png", b"two")],
            [("a.wav", b"one"), ("A.WAV/x.wav", b"two")],
            [("A.WAV/x.wav", b"one"), ("a.wav", b"two")],
        ] {
            let archive = workspace.0.join("collide.zip");
            write_zip(&archive, &entries.map(|(name, bytes)| (name, &bytes[..])));
            let error = library.reference_zip(&archive).unwrap_err();
            assert!(error.contains("Groß-/Kleinschreibung"), "{error}");
        }
        assert!(library.packages().unwrap().is_empty());

        let archive = workspace.0.join("pack.zip");
        write_zip(&archive, &[("hit.wav", b"hit audio")]);
        library.reference_zip(&archive).unwrap();
        let hit = file_id(&library, "hit.wav");
        let cached = library.resolve_preview(hit).unwrap();
        fs::write(&cached, b"HIT AUDIO").unwrap();
        assert_eq!(
            fs::read(library.resolve_preview(hit).unwrap()).unwrap(),
            b"hit audio",
            "same size, other CRC"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cache_cleanup_never_follows_a_link_out_of_the_library() {
        for link in [".cache", ".cache/zip"] {
            let workspace = Workspace::new();
            let archive = workspace.0.join("pack.zip");
            write_zip(&archive, &[("hit.wav", b"hit")]);
            let root = workspace.0.join("library");
            let mut library = Library::create(&root).unwrap();
            let package = library.reference_zip(&archive).unwrap();
            let outside = workspace.0.join("outside");
            let kept = outside
                .join(".cache")
                .join(&package.relative_path)
                .join("keep.txt");
            fs::create_dir_all(kept.parent().unwrap()).unwrap();
            fs::write(&kept, b"keep").unwrap();
            fs::create_dir_all(root.join(link).parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(outside.join(link), root.join(link)).unwrap();

            library.trash_package(package.id).unwrap();
            library.empty_trash().unwrap();

            assert!(kept.exists(), "deleted through {link}");
        }
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
