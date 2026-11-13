//! Unity packages (`.unitypackage`: a gzip tar of `<guid>/asset` payloads with their
//! `<guid>/pathname`) used in place like ZIPs (see `zip_sources`), for raw browsing of supported
//! assets only: prefabs, materials and scripts are not rebuilt. A tar has no directory, so the
//! GUID → path map comes from one bounded full scan; payloads are then read by GUID in one
//! forward pass over the decompressed stream.
use super::zip_sources::{archive_identity, crc_text, PathSet};
use super::*;
use flate2::read::GzDecoder;
use std::{
    collections::HashMap,
    io::{BufReader, Read},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub(super) const IDENTITY_PREFIX: &str = "unitypackage:";

pub(super) fn is_unity(identity: &str) -> bool {
    identity.starts_with(IDENTITY_PREFIX)
}

pub(super) fn is_unity_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("unitypackage"))
}

/// Bounds of the full scan; extraction passes read the same, unchanged archive.
pub(super) struct Limits {
    pub bytes: u64,
    pub entries: usize,
    pub time: Duration,
}
pub(super) const LIMITS: Limits = Limits {
    bytes: 64 << 30,
    entries: 1_000_000,
    time: Duration::from_secs(30 * 60),
};
const MAX_PATHNAME_FILE: u64 = 4096;
const MAX_PATH: usize = 1024;
const MAX_COMPONENT: usize = 255;

/// One asset payload in archive order.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Payload {
    pub path: PathBuf,
    pub guid: String,
    pub size: u64,
    pub crc: String,
}

/// Decompressed bytes from the archive start; fails past the byte budget or the deadline.
struct Stream {
    inner: GzDecoder<BufReader<fs::File>>,
    pos: u64,
    budget: u64,
    deadline: Option<Instant>,
}

impl Stream {
    fn open(path: &Path, budget: u64, time: Option<Duration>) -> Result<Self> {
        let file = fs::File::open(path).map_err(|e| format!("Unity-Paket nicht verfügbar: {e}"))?;
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            return Err("Bitte eine reguläre Unity-Paket-Datei auswählen".into());
        }
        Ok(Self {
            inner: GzDecoder::new(BufReader::with_capacity(1 << 16, file)),
            pos: 0,
            budget,
            deadline: time.map(|time| Instant::now() + time),
        })
    }

    /// Skips to the next 512-byte tar block boundary.
    fn align(&mut self) -> io::Result<()> {
        let padding = (512 - self.pos % 512) % 512;
        io::copy(&mut Read::take(&mut *self, padding), &mut io::sink()).map(drop)
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() > deadline)
        {
            return Err(io::Error::other(
                "Unity-Paket überschreitet die Zeitgrenze fürs Einlesen",
            ));
        }
        let read = self.inner.read(buffer)?;
        self.pos += read as u64;
        if self.pos > self.budget {
            return Err(io::Error::other(
                "Unity-Paket überschreitet die Grenze für entpackte Daten",
            ));
        }
        Ok(read)
    }
}

/// `<guid>/<file>` of a tar entry, or `None` for anything outside GUID folders.
fn guid_entry(name: &[u8]) -> Option<(&str, &str)> {
    let name = std::str::from_utf8(name).ok()?;
    let (guid, file) = name.strip_prefix("./").unwrap_or(name).split_once('/')?;
    (guid.len() == 32 && guid.bytes().all(|b| b.is_ascii_hexdigit())).then_some((guid, file))
}

fn entry_name<R: Read>(entry: &tar::Entry<'_, R>) -> Vec<u8> {
    entry.path_bytes().into_owned()
}

/// A Unity `pathname` checked for every OS: relative, no `.`/`..`, no backslash, drive, stream
/// or reserved character, no Windows device name or trailing dot/space, bounded lengths.
pub(super) fn checked_path(text: &str) -> Result<PathBuf> {
    const DEVICES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    let unsafe_part = |part: &str| {
        let stem = part
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_uppercase();
        let numbered = ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|n| {
                n.chars().count() == 1 && n.chars().all(|c| c.is_ascii_digit() || "¹²³".contains(c))
            })
        });
        part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > MAX_COMPONENT
            || part.ends_with(['.', ' '])
            || part.chars().any(|c| c < ' ' || r#"\:*?"<>|"#.contains(c))
            || DEVICES.contains(&stem.as_str())
            || numbered
    };
    if text.len() > MAX_PATH || text.split('/').any(unsafe_part) {
        return Err(format!("Unsicherer Dateipfad im Unity-Paket: {text}"));
    }
    Ok(PathBuf::from(text))
}

#[derive(Default)]
struct Record {
    order: usize,
    pathname: Option<String>,
    payload: Option<(u64, String)>,
}

/// Reads the whole archive within `limits` and lists its payloads in archive order. Rejects links
/// and special entries, duplicate GUID files, payloads without a path, supported files without a
/// payload, unsafe paths and paths that collide on case-insensitive disks.
pub(super) fn scan(source: &Path, limits: &Limits) -> Result<Vec<Payload>> {
    let failed = |e: io::Error| format!("Unity-Paket ist beschädigt oder nicht lesbar: {e}");
    let mut archive = tar::Archive::new(Stream::open(source, limits.bytes, Some(limits.time))?);
    let mut records: HashMap<String, Record> = HashMap::new();
    for (count, entry) in archive.entries().map_err(failed)?.enumerate() {
        if count >= limits.entries {
            return Err("Unity-Paket hat zu viele Einträge".into());
        }
        let mut entry = entry.map_err(failed)?;
        let name = entry_name(&entry);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(format!(
                "Verknüpfungen und spezielle Einträge werden nicht unterstützt: {}",
                String::from_utf8_lossy(&name)
            ));
        }
        let Some((guid, file @ ("asset" | "pathname"))) = guid_entry(&name) else {
            continue;
        };
        let record = records.entry(guid.to_owned()).or_default();
        let duplicate = || format!("Unity-Paket enthält die GUID {guid} mehrfach");
        if file == "pathname" {
            if record.pathname.is_some() {
                return Err(duplicate());
            }
            if entry.size() > MAX_PATHNAME_FILE {
                return Err(format!("Pfadname im Unity-Paket ist zu lang: {guid}"));
            }
            let mut text = String::new();
            entry.read_to_string(&mut text).map_err(failed)?;
            // Later lines hold Unity metadata (e.g. "00"); the first one is the path.
            let line = text.lines().next().unwrap_or("");
            record.pathname = Some(line.to_owned());
        } else {
            if record.payload.is_some() {
                return Err(duplicate());
            }
            let size = entry.size();
            let mut hasher = crc32fast::Hasher::new();
            let mut buffer = vec![0; 1 << 16];
            loop {
                match entry.read(&mut buffer).map_err(failed)? {
                    0 => break,
                    read => hasher.update(&buffer[..read]),
                }
            }
            record.order = count;
            record.payload = Some((size, crc_text(hasher.finalize())));
        }
    }
    let mut records: Vec<_> = records.into_iter().collect();
    records.sort_by_key(|(_, record)| record.order);
    let mut paths = PathSet::default();
    let mut payloads = Vec::new();
    for (guid, record) in records {
        let Some(pathname) = record.pathname else {
            return Err(format!("Unity-Paket: Eintrag {guid} hat keinen Pfad"));
        };
        let path = checked_path(&pathname)?;
        // A GUID without payload is a folder, unless its name is a supported asset.
        let folder = record.payload.is_none();
        if folder && assets::candidate_format(&path).is_some() {
            return Err(format!("Unity-Paket: Datei ohne Inhalt: {pathname}"));
        }
        if !paths.insert(&pathname, folder) {
            return Err(format!(
                "Unity-Paket enthält Pfade, die sich nur in Groß-/Kleinschreibung unterscheiden oder Datei und Ordner zugleich sind: {pathname}"
            ));
        }
        if let Some((size, crc)) = record.payload {
            payloads.push(Payload {
                path,
                guid,
                size,
                crc,
            });
        }
    }
    Ok(payloads)
}

/// Scans of recently used packages by archive identity, so model previews skip the full scan.
static INDEXES: Mutex<Vec<(String, Arc<Vec<Payload>>)>> = Mutex::new(Vec::new());

fn scan_checked(source: &Path) -> Result<(String, Arc<Vec<Payload>>)> {
    let identity = archive_identity(source)?;
    let payloads = Arc::new(scan(source, &LIMITS)?);
    if archive_identity(source)? != identity {
        return Err("Unity-Paket wurde während des Lesens verändert".into());
    }
    if let Ok(mut indexes) = INDEXES.lock() {
        indexes.retain(|(known, _)| *known != identity);
        indexes.push((identity.clone(), payloads.clone()));
        if indexes.len() > 4 {
            indexes.remove(0);
        }
    }
    Ok((identity, payloads))
}

/// Validates a package with a full scan and lists its supported files; the GUID is kept as the
/// file's `modified` origin, so payloads are found again without another scan.
pub(super) fn read_directory(source: &Path) -> Result<(String, Vec<ReferencedFile>)> {
    let (identity, payloads) = scan_checked(source)?;
    let files = payloads
        .iter()
        .filter_map(|payload| {
            let format = assets::candidate_format(&payload.path)?;
            Some(Ok(ReferencedFile {
                relative: payload.path.clone(),
                size: match i64::try_from(payload.size) {
                    Ok(size) => size,
                    Err(_) => return Some(Err("Datei ist zu groß für den Katalog".into())),
                },
                identity: payload.crc.clone(),
                modified: payload.guid.clone(),
                format: format.into(),
            }))
        })
        .collect::<Result<_>>()?;
    Ok((identity, files))
}

/// All payloads of the package whose archive has `identity`, scanning it when not remembered.
pub(super) fn payloads(source: &Path, identity: &str) -> Result<Arc<Vec<Payload>>> {
    if let Some((_, known)) = INDEXES
        .lock()
        .ok()
        .and_then(|indexes| indexes.iter().find(|(known, _)| known == identity).cloned())
    {
        return Ok(known);
    }
    let (current, payloads) = scan_checked(source)?;
    if current != identity {
        return Err("Unity-Paket wurde verschoben oder verändert. Bitte neu verknüpfen".into());
    }
    Ok(payloads)
}

/// Passes size and CRC errors to the reader at the payload's end.
struct Checked<'a, R> {
    inner: R,
    hasher: crc32fast::Hasher,
    read: u64,
    payload: &'a Payload,
}

impl<R: Read> Read for Checked<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        self.read += read as u64;
        if read == 0
            && (self.read != self.payload.size
                || crc_text(self.hasher.clone().finalize()) != self.payload.crc)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Eintrag im Unity-Paket wurde verändert. Bitte neu verknüpfen",
            ));
        }
        Ok(read)
    }
}

/// Forward reader over one package: requests in archive order share one decompression pass, an
/// earlier payload restarts from the archive start once.
pub(super) struct Cursor {
    source: PathBuf,
    stream: Option<Stream>,
}

impl Cursor {
    pub fn new(source: &Path) -> Self {
        Self {
            source: source.to_owned(),
            stream: None,
        }
    }

    /// Hands the payload's bytes (checked against its size and CRC) to `read`, then skips any
    /// rest of it. Reads stop with an error once `cancel` is set.
    pub fn read<T>(
        &mut self,
        payload: &Payload,
        cancel: &AtomicBool,
        read: impl FnOnce(&mut dyn Read) -> T,
    ) -> Result<T> {
        let mut read = Some(read);
        loop {
            let fresh = self.stream.is_none();
            if fresh {
                self.stream = Some(Stream::open(&self.source, LIMITS.bytes, None)?);
            }
            let stream = self.stream.as_mut().expect("opened above");
            match Self::find(stream, payload, cancel, &mut read) {
                Ok(Some(value)) => {
                    if stream.align().is_err() {
                        self.stream = None;
                    }
                    return Ok(value);
                }
                Ok(None) if !fresh => self.stream = None,
                Ok(None) => {
                    self.stream = None;
                    return Err("Eintrag fehlt im Unity-Paket. Bitte neu verknüpfen".into());
                }
                Err(error) => {
                    self.stream = None;
                    return Err(error);
                }
            }
        }
    }

    /// Advances to `<guid>/asset`; the tar reader starts at a block boundary each time.
    fn find<T>(
        stream: &mut Stream,
        payload: &Payload,
        cancel: &AtomicBool,
        read: &mut Option<impl FnOnce(&mut dyn Read) -> T>,
    ) -> Result<Option<T>> {
        let failed = |e: io::Error| format!("Unity-Paket nicht lesbar: {e}");
        let mut archive = tar::Archive::new(stream);
        for entry in archive.entries().map_err(failed)? {
            if cancel.load(Ordering::Relaxed) {
                return Err("abgebrochen".into());
            }
            let mut entry = entry.map_err(failed)?;
            if !entry.header().entry_type().is_file()
                || guid_entry(&entry_name(&entry)) != Some((&payload.guid, "asset"))
            {
                continue;
            }
            let mut checked = Checked {
                inner: zip_sources::Cancellable(cancel, &mut entry),
                hasher: crc32fast::Hasher::new(),
                read: 0,
                payload,
            };
            let value = read.take().expect("read once")(&mut checked);
            // The rest must be consumed so the next request starts at the following header.
            io::copy(&mut checked.inner, &mut io::sink()).map_err(failed)?;
            return Ok(Some(value));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };

    pub(crate) struct Workspace(pub PathBuf);
    impl Workspace {
        pub fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "lootbag-unity-{}-{}",
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

    /// Raw tar entries `(name, bytes)`; a `None` body adds a directory.
    pub(crate) fn write_tar(path: &Path, entries: &[(&str, Option<&[u8]>)]) {
        let mut tar = tar::Builder::new(GzEncoder::new(
            fs::File::create(path).unwrap(),
            Compression::fast(),
        ));
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            match bytes {
                Some(bytes) => {
                    header.set_size(bytes.len() as u64);
                    header.set_entry_type(tar::EntryType::Regular);
                    tar.append_data(&mut header, name, *bytes).unwrap();
                }
                None => {
                    header.set_size(0);
                    header.set_entry_type(tar::EntryType::Directory);
                    tar.append_data(&mut header, name, io::empty()).unwrap();
                }
            }
        }
        tar.into_inner().unwrap().finish().unwrap().flush().unwrap();
    }

    pub(crate) fn guid(n: usize) -> String {
        format!("{n:032x}")
    }

    /// A package of `(pathname, payload)` assets, in the usual Unity layout with metadata files.
    pub(crate) fn write_package(path: &Path, assets: &[(&str, Option<&[u8]>)]) {
        let names: Vec<_> = (0..assets.len()).map(guid).collect();
        let mut entries = Vec::new();
        for (guid, (pathname, payload)) in names.iter().zip(assets) {
            entries.push((format!("{guid}/"), None));
            if let Some(payload) = payload {
                entries.push((format!("{guid}/asset"), Some(payload.to_vec())));
            }
            entries.push((format!("{guid}/asset.meta"), Some(b"guid: x".to_vec())));
            entries.push((
                format!("{guid}/pathname"),
                Some(format!("{pathname}\n00").into_bytes()),
            ));
        }
        let entries: Vec<_> = entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_deref()))
            .collect();
        write_tar(path, &entries);
    }

    #[test]
    fn a_scan_maps_guids_to_paths_in_any_entry_order() {
        let workspace = Workspace::new();
        let package = workspace.0.join("pack.unitypackage");
        let (a, b) = (guid(1), guid(2));
        write_tar(
            &package,
            &[
                (&format!("./{a}/pathname"), Some(b"Assets/Sfx/hit.wav")),
                (&format!("{b}/asset"), Some(b"leaf")),
                (&format!("{a}/asset"), Some(b"hit")),
                (&format!("{b}/pathname"), Some(b"Assets/Tex/leaf.png\r\n00")),
                (&format!("{}/pathname", guid(3)), Some(b"Assets/Sfx")),
                ("stray.txt", Some(b"ignored")),
            ],
        );
        let payloads = scan(&package, &LIMITS).unwrap();
        let paths: Vec<_> = payloads.iter().map(|p| p.path.to_str().unwrap()).collect();
        assert_eq!(paths, ["Assets/Tex/leaf.png", "Assets/Sfx/hit.wav"]);
        assert_eq!(payloads[1].guid, a);
        assert_eq!(payloads[1].size, 3);

        let mut cursor = Cursor::new(&package);
        let cancel = AtomicBool::new(false);
        let text = |cursor: &mut Cursor, payload: &Payload| {
            cursor
                .read(payload, &cancel, |r| {
                    let mut text = String::new();
                    r.read_to_string(&mut text).map(|_| text)
                })
                .unwrap()
                .unwrap()
        };
        // Out of archive order: the cursor starts over for the earlier payload.
        assert_eq!(text(&mut cursor, &payloads[1]), "hit");
        assert_eq!(text(&mut cursor, &payloads[0]), "leaf");
        assert_eq!(text(&mut cursor, &payloads[1]), "hit");
        let changed = Payload {
            crc: crc_text(0),
            ..payloads[0].clone()
        };
        let error = cursor
            .read(&changed, &cancel, |r| r.read_to_end(&mut Vec::new()))
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("verändert"), "{error}");
        let missing = Payload {
            guid: guid(9),
            ..payloads[0].clone()
        };
        assert!(cursor.read(&missing, &cancel, |_| ()).is_err());
    }

    #[test]
    fn invalid_packages_are_rejected_whole() {
        let workspace = Workspace::new();
        let package = workspace.0.join("bad.unitypackage");
        let (a, b) = (guid(1), guid(2));
        type Entries = Vec<(String, Option<&'static [u8]>)>;
        let cases: Vec<(&str, Entries)> = vec![
            (
                "mehrfach",
                vec![
                    (format!("{a}/pathname"), Some(b"Assets/a.wav")),
                    (format!("{a}/asset"), Some(b"x")),
                    (format!("{a}/asset"), Some(b"y")),
                ],
            ),
            (
                "mehrfach",
                vec![
                    (format!("{a}/pathname"), Some(b"Assets/a.wav")),
                    (format!("{a}/pathname"), Some(b"Assets/b.wav")),
                ],
            ),
            ("keinen Pfad", vec![(format!("{a}/asset"), Some(b"x"))]),
            (
                "ohne Inhalt",
                vec![(format!("{a}/pathname"), Some(b"Assets/a.wav"))],
            ),
            (
                "Groß-/Kleinschreibung",
                vec![
                    (format!("{a}/pathname"), Some(b"Assets/Hit.wav")),
                    (format!("{a}/asset"), Some(b"x")),
                    (format!("{b}/pathname"), Some(b"assets/hit.WAV")),
                    (format!("{b}/asset"), Some(b"y")),
                ],
            ),
            (
                "Groß-/Kleinschreibung",
                vec![
                    (format!("{a}/pathname"), Some(b"Assets/a.wav")),
                    (format!("{a}/asset"), Some(b"x")),
                    (format!("{b}/pathname"), Some(b"Assets/A.WAV/b.wav")),
                    (format!("{b}/asset"), Some(b"y")),
                ],
            ),
            (
                "zu lang",
                vec![(format!("{a}/pathname"), Some(&[b'a'; 5000][..]))],
            ),
        ];
        for (expected, entries) in cases {
            let entries: Vec<_> = entries.iter().map(|(n, b)| (n.as_str(), *b)).collect();
            write_tar(&package, &entries);
            let error = scan(&package, &LIMITS).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        }

        // Links and special entries.
        for kind in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Fifo,
            tar::EntryType::Char,
        ] {
            let mut tar = tar::Builder::new(GzEncoder::new(
                fs::File::create(&package).unwrap(),
                Compression::fast(),
            ));
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_size(0);
            tar.append_link(&mut header, format!("{a}/asset"), "../../x")
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
            let error = scan(&package, &LIMITS).unwrap_err();
            assert!(error.contains("Verknüpfungen"), "{kind:?}: {error}");
        }

        fs::write(&package, b"not gzip").unwrap();
        assert!(scan(&package, &LIMITS).is_err());
    }

    #[test]
    fn unsafe_and_windows_hostile_paths_are_rejected() {
        let long_component = "a".repeat(256);
        let long_path = format!("Assets/{}", "abcdefgh/".repeat(128));
        for path in [
            "",
            "/Assets/a.wav",
            "Assets/../a.wav",
            "Assets/./a.wav",
            "Assets//a.wav",
            "..\\a.wav",
            "Assets\\a.wav",
            "C:/a.wav",
            "C:a.wav",
            "\\\\server\\share\\a.wav",
            "//server/share/a.wav",
            "Assets/a.wav:stream",
            "Assets/CON",
            "Assets/con.wav",
            "Assets/Nul .png",
            "Assets/COM1.ogg",
            "Assets/lpt9/a.wav",
            "Assets/COM¹.wav",
            "Assets/dot./a.wav",
            "Assets/space /a.wav",
            "Assets/a.wav.",
            "Assets/a?.wav",
            "Assets/a\u{1}.wav",
            &format!("Assets/{long_component}.wav"),
            &long_path,
        ] {
            assert!(checked_path(path).is_err(), "accepted {path:?}");
        }
        for path in [
            "Assets/a.wav",
            "Assets/Console/con1.wav",
            "Assets/COM10.wav",
            "Assets/Aux Sounds/x.png",
            "Packages/com.unity/ä.png",
        ] {
            assert!(checked_path(path).is_ok(), "rejected {path:?}");
        }
    }

    fn files_below(path: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            if entry.file_type().unwrap().is_dir() {
                found.extend(files_below(&entry.path()));
            } else {
                found.push(entry.path());
            }
        }
        found
    }

    fn file_id(library: &Library, name: &str) -> (i64, i64) {
        library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .iter()
            .flat_map(|asset| asset.files.iter().map(move |file| (asset.id, file)))
            .find(|(_, file)| file.original_filename == name)
            .map(|(asset, file)| (asset, file.id))
            .unwrap_or_else(|| panic!("{name} not catalogued"))
    }

    const GLTF: &[u8] =
        br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"tree.bin"}],"images":[{"uri":"tex/leaf.png"}]}"#;

    fn forest(workspace: &Workspace) -> PathBuf {
        let package = workspace.0.join("Forest.unitypackage");
        write_package(
            &package,
            &[
                ("Assets/Sfx", None),
                ("Assets/Sfx/hit.wav", Some(b"hit audio")),
                ("Assets/Sfx/copy.wav", Some(b"hit audio")),
                ("Assets/Tree/tree.gltf", Some(GLTF)),
                ("Assets/Tree/tree.bin", Some(b"mesh bytes")),
                ("Assets/Tree/tex/leaf.png", Some(b"leaf png")),
                ("Assets/Tree/Tree.prefab", Some(b"prefab yaml")),
                ("Assets/Scripts/Tree.cs", Some(b"class Tree {}")),
            ],
        );
        package
    }

    #[test]
    fn a_unity_package_is_browsed_previewed_and_exported_like_a_zip() {
        let workspace = Workspace::new();
        let package = forest(&workspace);
        let package_bytes = fs::read(&package).unwrap();
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();

        let referenced = library.reference_zip(&package).unwrap();

        assert_eq!(referenced.name, "Forest");
        assert!(referenced.zip && referenced.unity);
        assert!(library.packages().unwrap()[0].unity);
        let mut names: Vec<_> = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .iter()
            .flat_map(|a| a.files.iter().map(|f| f.original_filename.clone()))
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["copy.wav", "hit.wav", "leaf.png", "tree.gltf"],
            "only supported assets, no prefabs or scripts"
        );
        assert_eq!(files_below(&root), vec![root.join("catalog.sqlite3")]);

        let (hit_asset, hit) = file_id(&library, "hit.wav");
        let preview = library.resolve_preview(hit).unwrap();
        assert!(preview.starts_with(root.join(".cache/unity")));
        assert!(preview.ends_with("Assets/Sfx/hit.wav"));
        assert_eq!(fs::read(&preview).unwrap(), b"hit audio");
        assert_eq!(library.resolve_preview(hit).unwrap(), preview);
        let export = workspace.0.join("out.wav");
        library.export(hit, Some(&export), false).unwrap();
        assert_eq!(fs::read(&export).unwrap(), b"hit audio");

        let (_, model) = file_id(&library, "tree.gltf");
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
            [
                "Assets/Tree/tex/leaf.png",
                "Assets/Tree/tree.bin",
                "Assets/Tree/tree.gltf"
            ]
        );
        assert!(!files_below(&root.join(".cache"))
            .iter()
            .any(|p| p.ends_with("Tree.prefab")));

        let game = library.create_folder("Game", None).unwrap();
        library.add_to_folder(game.id, hit_asset).unwrap();
        let exports = workspace.0.join("exports");
        fs::create_dir(&exports).unwrap();
        library.export_project(game.id, &exports).unwrap();
        assert!(files_below(&exports.join("Game"))
            .iter()
            .any(|p| fs::read(p).unwrap() == b"hit audio"));

        let error = library.rescan_package(referenced.id).unwrap_err();
        assert!(error.contains("Unity-Pakete"), "{error}");
        library.trash_package(referenced.id).unwrap();
        library.empty_trash().unwrap();
        assert!(!root.join(".cache").join(&referenced.relative_path).exists());
        assert_eq!(fs::read(&package).unwrap(), package_bytes);
    }

    #[test]
    fn hashing_and_model_traits_read_a_unity_package_without_caching() {
        let workspace = Workspace::new();
        let package = forest(&workspace);
        let root = workspace.0.join("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_zip(&package).unwrap();
        let cancel = AtomicBool::new(false);

        let (hashed, ..) = library.hash_pending_files(&mut 0, 100, &cancel).unwrap();
        let (inspected, _) = library
            .inspect_pending_models(&mut 0, 100, &cancel)
            .unwrap();

        assert_eq!((hashed, inspected), (2, 1));
        let items = library.query(CatalogQuery::default()).unwrap().items;
        let duplicates = |name: &str| items.iter().find(|a| a.name == name).unwrap().duplicates;
        assert_eq!((duplicates("hit"), duplicates("copy")), (1, 1));
        assert!(files_below(&root.join(".cache")).is_empty());
    }

    #[test]
    fn a_changed_package_is_offline_and_drops_its_cache() {
        let workspace = Workspace::new();
        let package = forest(&workspace);
        let mut library = Library::create(&workspace.0.join("library")).unwrap();
        library.reference_zip(&package).unwrap();
        let (_, hit) = file_id(&library, "hit.wav");
        let cached = library.resolve_preview(hit).unwrap();

        write_package(&package, &[("Assets/Sfx/hit.wav", Some(b"other"))]);

        assert!(!library.packages().unwrap()[0].available);
        let error = library.resolve_preview(hit).unwrap_err();
        assert!(error.contains("Unity-Paket"), "{error}");
        assert!(!cached.exists());
    }

    /// Cold-read latency on a realistic large package: 1 GiB of payloads, half incompressible (as
    /// compressed audio and textures), half low-entropy (as WAV or FBX):
    /// `cargo test --release --lib -- --ignored --nocapture cold_read`.
    #[test]
    #[ignore]
    fn cold_read_latency() {
        let workspace = Workspace::new();
        let package = workspace.0.join("Large.unitypackage");
        let mut tar = tar::Builder::new(GzEncoder::new(
            io::BufWriter::new(fs::File::create(&package).unwrap()),
            Compression::default(),
        ));
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut chunk = vec![0u8; 256 << 10];
        let count = 4096;
        for index in 0..count {
            for byte in chunk.iter_mut() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *byte = if index % 2 == 0 {
                    seed as u8
                } else {
                    seed as u8 & 0x0f
                };
            }
            let guid = guid(index);
            let mut add = |name: String, bytes: &[u8]| {
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o644);
                header.set_size(bytes.len() as u64);
                tar.append_data(&mut header, name, bytes).unwrap();
            };
            add(format!("{guid}/asset"), &chunk);
            add(format!("{guid}/asset.meta"), b"fileFormatVersion: 2");
            add(
                format!("{guid}/pathname"),
                format!("Assets/Audio/sound_{index:04}.ogg").as_bytes(),
            );
        }
        tar.into_inner().unwrap().finish().unwrap();
        let size = fs::metadata(&package).unwrap().len();
        let mut library = Library::create(&workspace.0.join("library")).unwrap();

        let started = Instant::now();
        library.reference_zip(&package).unwrap();
        let scanned = started.elapsed();
        let id = |name: String| -> i64 {
            library
                .connection
                .query_row(
                    "SELECT id FROM asset_files WHERE relative_path LIKE '%/' || ?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let first = id("sound_0000.ogg".into());
        let last = id(format!("sound_{:04}.ogg", count - 1));
        let started = Instant::now();
        library.resolve_preview(first).unwrap();
        let first_read = started.elapsed();
        let started = Instant::now();
        library.resolve_preview(last).unwrap();
        let last_read = started.elapsed();
        let started = Instant::now();
        library.resolve_preview(last).unwrap();
        let cached_read = started.elapsed();
        eprintln!(
            "{count} payloads, {} MiB archive: scan {scanned:?}, cold first {first_read:?}, cold last {last_read:?}, cached {cached_read:?}",
            size >> 20
        );
    }

    #[test]
    fn scans_stop_at_the_byte_entry_and_time_bounds() {
        let workspace = Workspace::new();
        let package = workspace.0.join("big.unitypackage");
        write_package(
            &package,
            &[
                ("Assets/a.wav", Some(&[0; 64 << 10])),
                ("Assets/b.wav", Some(b"b")),
            ],
        );
        assert!(scan(&package, &LIMITS).is_ok());
        let bounded = |bytes, entries, time| {
            scan(
                &package,
                &Limits {
                    bytes,
                    entries,
                    time,
                },
            )
            .unwrap_err()
        };
        let hour = Duration::from_secs(3600);
        assert!(bounded(32 << 10, 100, hour).contains("entpackte Daten"));
        assert!(bounded(1 << 30, 4, hour).contains("zu viele Einträge"));
        assert!(bounded(1 << 30, 100, Duration::ZERO).contains("Zeitgrenze"));
    }
}
