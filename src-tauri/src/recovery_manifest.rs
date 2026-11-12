use crate::library::{storage::Stamp, ImportControl, Result};
use serde::{
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
    Deserialize, Serialize,
};
use std::{
    fmt,
    fs::File,
    io::{self, BufReader, Read, Seek, Write},
    os::unix::fs::FileExt,
    path::{Component, Path},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PAYLOAD: u64 = 2 * 1024 * 1024 * 1024 * 1024;
const MAX_SAFE: u64 = 9_007_199_254_740_991;
const BUFFER: usize = 64 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) catalog_version: u32,
    pub(super) external_coverage: String,
    pub(super) external_reference_records: u64,
    pub(super) entries: Vec<ManifestEntry>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ManifestEntry {
    pub(super) path: String,
    pub(super) kind: EntryKind,
    pub(super) bytes: u64,
    pub(super) sha256: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum EntryKind {
    File,
    Directory,
}

fn checkpoint(control: &ImportControl, started: Instant, work: Duration) -> Result<()> {
    if control.cancellation_flag().load(Ordering::Relaxed) {
        return Err("Manifest-Vorgang wurde abgebrochen".into());
    }
    if started.elapsed() >= work {
        return Err("Manifest-Zeitlimit überschritten".into());
    }
    Ok(())
}
fn checked_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.split('/').count() > 64
        || path.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.len() > 255
                || part.contains(['\\', '\0'])
        })
        || !Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("Manifest enthält einen unsicheren oder zu langen Pfad".into());
    }
    Ok(())
}
fn validate(
    manifest: &Manifest,
    control: &ImportControl,
    started: Instant,
    work: Duration,
) -> Result<()> {
    checkpoint(control, started, work)?;
    if manifest.format != "game-asset-library-backup"
        || manifest.version != 1
        || manifest.catalog_version != 1
        || manifest.external_coverage != "metadataOnly"
        || manifest.external_reference_records > MAX_SAFE
    {
        return Err("Nicht unterstütztes Manifestformat".into());
    }
    let mut total = 0_u64;
    let mut previous: Option<&str> = None;
    for entry in &manifest.entries {
        checkpoint(control, started, work)?;
        checked_path(&entry.path)?;
        if previous.is_some_and(|path| path >= entry.path.as_str()) {
            return Err("Manifest-Pfade sind doppelt oder nicht kanonisch sortiert".into());
        }
        previous = Some(&entry.path);
        match entry.kind {
            EntryKind::Directory if entry.bytes != 0 || entry.sha256.is_some() => {
                return Err("Ungültiger Manifest-Ordner".into())
            }
            EntryKind::File => {
                if !entry.sha256.as_deref().is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                }) {
                    return Err("Ungültiger SHA256 im Manifest".into());
                }
                total = total
                    .checked_add(entry.bytes)
                    .ok_or("Manifest-Nutzdatenlimit überschritten")?;
                if total > MAX_PAYLOAD {
                    return Err("Manifest-Nutzdatenlimit überschritten".into());
                }
            }
            EntryKind::Directory => {}
        }
        if let Some((parent, _)) = entry.path.rsplit_once('/') {
            let index = manifest
                .entries
                .binary_search_by(|candidate| candidate.path.as_str().cmp(parent))
                .map_err(|_| "Manifest-Elternordner fehlt")?;
            if manifest.entries[index].kind != EntryKind::Directory {
                return Err("Manifest-Elternpfad ist keine Verzeichnisstruktur".into());
            }
        }
    }
    Ok(())
}
struct Budget<'a> {
    retained: usize,
    maximum: usize,
    count: usize,
    max_entries: usize,
    control: &'a ImportControl,
    started: Instant,
    work: Duration,
}
impl Budget<'_> {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        checkpoint(self.control, self.started, self.work)?;
        self.retained = self
            .retained
            .checked_add(bytes)
            .ok_or("Manifest-Speicherlimit überschritten")?;
        if self.retained > self.maximum {
            return Err("Manifest-Speicherlimit überschritten".into());
        }
        Ok(())
    }
}
struct Text<'a, 'b> {
    budget: &'a mut Budget<'b>,
    maximum: usize,
}
impl<'de> DeserializeSeed<'de> for Text<'_, '_> {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<String, D::Error> {
        deserializer.deserialize_str(self)
    }
}
impl<'de> Visitor<'de> for Text<'_, '_> {
    type Value = String;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("bounded manifest string")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<String, E> {
        if value.len() > self.maximum {
            return Err(E::custom("Manifest-Zeichenfolgenlimit überschritten"));
        }
        self.budget.charge(value.len()).map_err(E::custom)?;
        Ok(value.to_owned())
    }
}
struct OptionalText<'a, 'b> {
    budget: &'a mut Budget<'b>,
}
impl<'de> DeserializeSeed<'de> for OptionalText<'_, '_> {
    type Value = Option<String>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_option(self)
    }
}
impl<'de> Visitor<'de> for OptionalText<'_, '_> {
    type Value = Option<String>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("null or bounded SHA256")
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_some<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        Text {
            budget: self.budget,
            maximum: 64,
        }
        .deserialize(deserializer)
        .map(Some)
    }
}
struct Number;
impl<'de> DeserializeSeed<'de> for Number {
    type Value = u64;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<u64, D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Number {
    type Value = u64;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("unsigned manifest integer")
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<u64, E> {
        Ok(value)
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<u64, E> {
        value
            .try_into()
            .map_err(|_| E::custom("Negative Manifest-Zahl"))
    }
    fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<u64, E> {
        Err(E::custom("Manifest-Zahl muss eine Ganzzahl sein"))
    }
}
#[derive(Clone, Copy)]
enum Key {
    Format,
    Version,
    CatalogVersion,
    ExternalCoverage,
    ExternalReferenceRecords,
    Entries,
    Path,
    Kind,
    Bytes,
    Sha256,
}
struct KeySeed;
impl<'de> DeserializeSeed<'de> for KeySeed {
    type Value = Key;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Key, D::Error> {
        deserializer.deserialize_identifier(self)
    }
}
impl<'de> Visitor<'de> for KeySeed {
    type Value = Key;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("known manifest field")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Key, E> {
        match value {
            "format" => Ok(Key::Format),
            "version" => Ok(Key::Version),
            "catalogVersion" => Ok(Key::CatalogVersion),
            "externalCoverage" => Ok(Key::ExternalCoverage),
            "externalReferenceRecords" => Ok(Key::ExternalReferenceRecords),
            "entries" => Ok(Key::Entries),
            "path" => Ok(Key::Path),
            "kind" => Ok(Key::Kind),
            "bytes" => Ok(Key::Bytes),
            "sha256" => Ok(Key::Sha256),
            _ => Err(E::custom("Unbekanntes Feld im Manifest")),
        }
    }
}
macro_rules! field {
    ($slot:ident, $name:literal, $value:expr) => {
        if $slot.is_some() {
            return Err(de::Error::duplicate_field($name));
        }
        $slot = Some($value?);
    };
}
struct EntrySeed<'a, 'b> {
    budget: &'a mut Budget<'b>,
}
impl<'de> DeserializeSeed<'de> for EntrySeed<'_, '_> {
    type Value = ManifestEntry;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        if self.budget.count >= self.budget.max_entries {
            return Err(de::Error::custom("Manifest-Anzahllimit überschritten"));
        }
        // Four-entry allowance also covers Vec's initial minimum capacity before the first push.
        self.budget
            .charge(4 * std::mem::size_of::<ManifestEntry>() + 64)
            .map_err(de::Error::custom)?;
        self.budget.count += 1;
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for EntrySeed<'_, '_> {
    type Value = ManifestEntry;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("bounded manifest entry")
    }
    fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<Self::Value, E> {
        Err(E::custom("Manifest-Eintrag muss ein Objekt sein"))
    }
    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let (mut path, mut kind, mut bytes, mut sha256) = (None, None, None, None);
        while let Some(key) = map.next_key_seed(KeySeed)? {
            match key {
                Key::Path => {
                    field!(
                        path,
                        "path",
                        map.next_value_seed(Text {
                            budget: &mut *self.budget,
                            maximum: 4096
                        })
                    );
                }
                Key::Kind => {
                    field!(
                        kind,
                        "kind",
                        map.next_value_seed(Text {
                            budget: &mut *self.budget,
                            maximum: 9
                        })
                    );
                }
                Key::Bytes => {
                    field!(bytes, "bytes", map.next_value_seed(Number));
                }
                Key::Sha256 => {
                    field!(
                        sha256,
                        "sha256",
                        map.next_value_seed(OptionalText {
                            budget: &mut *self.budget
                        })
                    );
                }
                _ => return Err(de::Error::custom("Unbekanntes Feld im Manifest-Eintrag")),
            }
        }
        let kind = match kind
            .as_deref()
            .ok_or_else(|| de::Error::missing_field("kind"))?
        {
            "file" => EntryKind::File,
            "directory" => EntryKind::Directory,
            _ => return Err(de::Error::custom("Unbekannter Manifest-Eintragstyp")),
        };
        Ok(ManifestEntry {
            path: path.ok_or_else(|| de::Error::missing_field("path"))?,
            kind,
            bytes: bytes.ok_or_else(|| de::Error::missing_field("bytes"))?,
            sha256: sha256.unwrap_or(None),
        })
    }
}
struct Entries<'a, 'b> {
    budget: &'a mut Budget<'b>,
}
impl<'de> DeserializeSeed<'de> for Entries<'_, '_> {
    type Value = Vec<ManifestEntry>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Entries<'_, '_> {
    type Value = Vec<ManifestEntry>;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("bounded manifest entries")
    }
    fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<Self::Value, E> {
        Err(E::custom("Manifest-Einträge müssen eine Liste sein"))
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut sequence: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = sequence.next_element_seed(EntrySeed {
            budget: &mut *self.budget,
        })? {
            entries.try_reserve(1).map_err(|_| {
                de::Error::custom("Manifest-Speicher konnte nicht reserviert werden")
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }
}
struct ManifestSeed<'a, 'b> {
    budget: &'a mut Budget<'b>,
}
impl<'de> DeserializeSeed<'de> for ManifestSeed<'_, '_> {
    type Value = Manifest;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Manifest, D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for ManifestSeed<'_, '_> {
    type Value = Manifest;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("versioned bounded manifest")
    }
    fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<Self::Value, E> {
        Err(E::custom("Manifest muss ein Objekt sein"))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Manifest, A::Error> {
        let (mut format, mut version, mut catalog, mut coverage, mut external, mut entries) =
            (None, None, None, None, None, None);
        while let Some(key) = map.next_key_seed(KeySeed)? {
            match key {
                Key::Format => {
                    field!(
                        format,
                        "format",
                        map.next_value_seed(Text {
                            budget: &mut *self.budget,
                            maximum: 32
                        })
                    );
                }
                Key::Version => {
                    field!(version, "version", map.next_value_seed(Number));
                }
                Key::CatalogVersion => {
                    field!(catalog, "catalogVersion", map.next_value_seed(Number));
                }
                Key::ExternalCoverage => {
                    field!(
                        coverage,
                        "externalCoverage",
                        map.next_value_seed(Text {
                            budget: &mut *self.budget,
                            maximum: 32
                        })
                    );
                }
                Key::ExternalReferenceRecords => {
                    field!(
                        external,
                        "externalReferenceRecords",
                        map.next_value_seed(Number)
                    );
                }
                Key::Entries => {
                    field!(
                        entries,
                        "entries",
                        map.next_value_seed(Entries {
                            budget: &mut *self.budget
                        })
                    );
                }
                _ => return Err(de::Error::custom("Unbekanntes Feld im Manifest")),
            }
        }
        Ok(Manifest {
            format: format.ok_or_else(|| de::Error::missing_field("format"))?,
            version: u32::try_from(version.ok_or_else(|| de::Error::missing_field("version"))?)
                .map_err(|_| de::Error::custom("Manifestversion überschreitet das Limit"))?,
            catalog_version: u32::try_from(
                catalog.ok_or_else(|| de::Error::missing_field("catalogVersion"))?,
            )
            .map_err(|_| de::Error::custom("Katalogversion überschreitet das Limit"))?,
            external_coverage: coverage
                .ok_or_else(|| de::Error::missing_field("externalCoverage"))?,
            external_reference_records: external
                .ok_or_else(|| de::Error::missing_field("externalReferenceRecords"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}
struct BorrowedReader<'a> {
    file: &'a File,
    offset: u64,
    length: u64,
    control: &'a ImportControl,
    started: Instant,
    work: Duration,
}
impl Read for BorrowedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        checkpoint(self.control, self.started, self.work).map_err(io::Error::other)?;
        let bytes = buffer.len().min((self.length - self.offset) as usize);
        let read = self.file.read_at(&mut buffer[..bytes], self.offset)?;
        self.offset += read as u64;
        Ok(read)
    }
}
pub(super) fn read(
    file: &File,
    max_bytes: u64,
    max_entries: usize,
    max_retained: usize,
    control: &ImportControl,
    started: Instant,
    work: Duration,
) -> Result<Manifest> {
    checkpoint(control, started, work)?;
    if max_bytes == 0 || max_bytes > MAX_BYTES || max_entries > 250_000 {
        return Err("Nicht unterstütztes Manifest-Limit".into());
    }
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err("Manifest-Dateigrößenlimit überschritten".into());
    }
    let stamp = Stamp::read(&metadata);
    let scratch = usize::try_from(max_bytes)
        .map_err(|_| "Manifest-Speicherlimit überschritten")?
        .checked_next_power_of_two()
        .ok_or("Manifest-Speicherlimit überschritten")?
        .max(8);
    let mut budget = Budget {
        retained: 0,
        maximum: max_retained,
        count: 0,
        max_entries,
        control,
        started,
        work,
    };
    // Known retained buffers and conservative entry/string capacities, not a process RSS measurement.
    budget.charge(
        scratch
            .checked_add(BUFFER + 512)
            .ok_or("Manifest-Speicherlimit überschritten")?,
    )?;
    let reader = BufReader::with_capacity(
        BUFFER,
        BorrowedReader {
            file,
            offset: 0,
            length: metadata.len(),
            control,
            started,
            work,
        },
    );
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let manifest = ManifestSeed {
        budget: &mut budget,
    }
    .deserialize(&mut deserializer)
    .map_err(|e| e.to_string())?;
    deserializer.end().map_err(|e| e.to_string())?;
    validate(&manifest, control, started, work)?;
    if Stamp::read(&file.metadata().map_err(|e| e.to_string())?) != stamp {
        return Err("Manifest-Datei wurde beim Lesen verändert".into());
    }
    Ok(manifest)
}
struct LimitedWriter<'a> {
    file: &'a mut File,
    written: u64,
    maximum: u64,
    control: &'a ImportControl,
    started: Instant,
    work: Duration,
}
impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        checkpoint(self.control, self.started, self.work).map_err(io::Error::other)?;
        if bytes.len() as u64 > self.maximum - self.written {
            return Err(io::Error::other("Manifest-Dateigrößenlimit überschritten"));
        }
        let count = self.file.write(bytes)?;
        self.written += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
pub(super) fn write(
    file: &mut File,
    manifest: &Manifest,
    max_bytes: u64,
    control: &ImportControl,
    started: Instant,
    work: Duration,
) -> Result<()> {
    checkpoint(control, started, work)?;
    if max_bytes == 0 || max_bytes > MAX_BYTES || manifest.entries.len() > 250_000 {
        return Err("Nicht unterstütztes Manifest-Limit".into());
    }
    validate(manifest, control, started, work)?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.len() != 0
        || file.stream_position().map_err(|e| e.to_string())? != 0
    {
        return Err("Manifest-Ausgabe muss eine neue leere Datei sein".into());
    }
    {
        let mut writer = LimitedWriter {
            file,
            written: 0,
            maximum: max_bytes,
            control,
            started,
            work,
        };
        serde_json::to_writer(&mut writer, manifest).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    file.sync_all().map_err(|e| e.to_string())?;
    checkpoint(control, started, work)
}

#[cfg(test)]
mod tests {
    use super::{read, write};
    use crate::library::{unique_suffix, ImportControl};
    use std::{
        fs::{self, File},
        path::PathBuf,
        time::{Duration, Instant},
    };

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            let path = PathBuf::from("/private/tmp")
                .join(format!("game-assets-recovery-manifest-{}", unique_suffix()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn valid() -> String {
        format!(
            r#"{{"format":"game-asset-library-backup","version":1,"catalogVersion":1,"externalCoverage":"metadataOnly","externalReferenceRecords":0,"entries":[{{"path":"a","kind":"directory","bytes":0,"sha256":null}},{{"path":"a/sound.wav","kind":"file","bytes":3,"sha256":"{}"}}]}}"#,
            "0".repeat(64)
        )
    }
    #[test]
    fn recovery_manifest_round_trip_keeps_input_bytes_and_canonical_entries() {
        let workspace = Workspace::new();
        let source = workspace.0.join("manifest.json");
        let bytes = valid().into_bytes();
        fs::write(&source, &bytes).unwrap();
        let control = ImportControl::default();
        let file = File::open(&source).unwrap();
        let manifest = read(
            &file,
            1024,
            4,
            131072,
            &control,
            Instant::now(),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(manifest.entries[0].path, "a");
        assert_eq!(manifest.entries[1].path, "a/sound.wav");
        assert_eq!(fs::read(&source).unwrap(), bytes);
        let copy = workspace.0.join("copy.json");
        let mut output = File::create(&copy).unwrap();
        write(
            &mut output,
            &manifest,
            1024,
            &control,
            Instant::now(),
            Duration::from_secs(1),
        )
        .unwrap();
        drop(output);
        let file = File::open(&copy).unwrap();
        let copy = read(
            &file,
            1024,
            4,
            131072,
            &control,
            Instant::now(),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            copy.entries[1].sha256.as_deref(),
            Some("0000000000000000000000000000000000000000000000000000000000000000")
        );
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }
    fn refusal(text: &str, count: usize, retained: usize) -> String {
        let workspace = Workspace::new();
        let source = workspace.0.join("refused.json");
        fs::write(&source, text).unwrap();
        let file = File::open(&source).unwrap();
        let error = read(
            &file,
            8192,
            count,
            retained,
            &ImportControl::default(),
            Instant::now(),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(fs::read(&source).unwrap(), text.as_bytes());
        error
    }
    #[test]
    fn recovery_manifest_entry_and_memory_limits_precede_later_malformed_input() {
        let malformed = valid().replace(r#"{"path":"a/sound.wav""#, "BROKEN_LATER_VALUE");
        let error = refusal(&malformed, 1, 262144);
        assert!(error.contains("Anzahllimit"), "{error}");
        let first_bad = valid().replace(r#"[{"path":"a""#, "[BROKEN_FIRST_VALUE");
        let workspace = Workspace::new();
        let source = workspace.0.join("memory.json");
        fs::write(&source, &first_bad).unwrap();
        let file = File::open(&source).unwrap();
        let error = read(
            &file,
            1024,
            4,
            67200,
            &ImportControl::default(),
            Instant::now(),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.contains("Speicherlimit"), "{error}");
        assert_eq!(fs::read(&source).unwrap(), first_bad.as_bytes());
    }
    #[test]
    fn recovery_manifest_refuses_unknown_unsafe_duplicate_missing_parent_and_truncated_documents() {
        let base = valid();
        let duplicate_path = base.replace("a/sound.wav", "a");
        let missing_parent = base.replace("a/sound.wav", "missing/sound.wav");
        let file_parent = base.replace(
            r#""path":"a","kind":"directory","bytes":0,"sha256":null"#,
            &format!(
                r#""path":"a","kind":"file","bytes":0,"sha256":"{}""#,
                "0".repeat(64)
            ),
        );
        let unsorted = base.replace(r#""path":"a""#, r#""path":"z""#);
        for text in [
            base.replace(r#""version":1"#, r#""version":2"#),
            base.replace(r#""catalogVersion":1"#, r#""catalogVersion":2"#),
            base.replace("metadataOnly", "complete"),
            base.replace("game-asset-library-backup", "other-format"),
            base.replace(r#""version":1"#, r#""version":1,"version":1"#),
            base.replace(r#""bytes":3"#, r#""bytes":3,"extra":true"#),
            base.replace(r#""path":"a""#, r#""path":"../a""#),
            base.replace("a/sound.wav", "a//sound.wav"),
            base.replace("a/sound.wav", "a/./sound.wav"),
            base.replace("a/sound.wav", "a/../sound.wav"),
            base.replace("a/sound.wav", r"a\\sound.wav"),
            base.replace("a/sound.wav", r"a/\u0000sound.wav"),
            base.replace("a/sound.wav", &format!("a/{}", "x".repeat(256))),
            base.replace("a/sound.wav", &vec!["a"; 65].join("/")),
            base.replace(&"0".repeat(64), &"A".repeat(64)),
            base.replace(
                r#""kind":"directory","bytes":0"#,
                r#""kind":"directory","bytes":1"#,
            ),
            duplicate_path,
            missing_parent,
            file_parent,
            unsorted,
            base[..base.len() - 1].to_owned(),
            format!("{base} {{}}"),
        ] {
            assert!(!refusal(&text, 8, 262144).is_empty());
        }
        let wrong_entry = base.replace(r#"[{"path":"a""#, &format!("[\"{}\"", "x".repeat(4096)));
        assert!(refusal(&wrong_entry, 8, 262144).len() < 256);
        let wrong_entries = base.replace(
            r#""entries":["#,
            &format!("\"entries\":\"{}\"", "x".repeat(4096)),
        );
        assert!(refusal(&wrong_entries, 8, 262144).len() < 256);
        assert!(refusal(&format!("\"{}\"", "x".repeat(4096)), 8, 262144).len() < 256);
        let huge_key = format!("{{\"{}\":null}}", "x".repeat(4096));
        assert!(refusal(&huge_key, 8, 262144).len() < 256);
        let numeric_string = base.replace(
            r#""externalReferenceRecords":0"#,
            &format!(r#""externalReferenceRecords":"{}""#, "x".repeat(4096)),
        );
        assert!(refusal(&numeric_string, 8, 262144).len() < 256);
    }
    #[test]
    fn recovery_manifest_bounds_logical_payload_bytes_input_length_and_stop() {
        let workspace = Workspace::new();
        let source = workspace.0.join("limits.json");
        let large = valid().replace(r#""bytes":3"#, r#""bytes":4294967297"#);
        fs::write(&source, &large).unwrap();
        let file = File::open(&source).unwrap();
        let control = ImportControl::default();
        let manifest = read(
            &file,
            1024,
            4,
            131072,
            &control,
            Instant::now(),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(manifest.entries[1].bytes, 4294967297);
        let error = read(
            &file,
            16,
            4,
            131072,
            &control,
            Instant::now(),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.contains("Dateigrößenlimit"));
        assert!(read(
            &file,
            1024,
            4,
            131072,
            &control,
            Instant::now(),
            Duration::ZERO
        )
        .unwrap_err()
        .contains("Zeitlimit"));
        control.cancel();
        assert!(read(
            &file,
            1024,
            4,
            131072,
            &control,
            Instant::now(),
            Duration::from_secs(1)
        )
        .unwrap_err()
        .contains("abgebrochen"));
        assert_eq!(fs::read(&source).unwrap(), large.as_bytes());
        assert!(refusal(
            &valid().replace(r#""bytes":3"#, r#""bytes":2199023255553"#),
            4,
            262144
        )
        .contains("Nutzdatenlimit"));
        assert!(refusal(
            &valid().replace(
                r#""externalReferenceRecords":0"#,
                r#""externalReferenceRecords":9007199254740992"#
            ),
            4,
            262144
        )
        .contains("Manifestformat"));
    }
    #[test]
    fn recovery_manifest_writer_bounds_output_and_keeps_existing_files() {
        let workspace = Workspace::new();
        let source = workspace.0.join("valid.json");
        fs::write(&source, valid()).unwrap();
        let file = File::open(&source).unwrap();
        let control = ImportControl::default();
        let mut manifest = read(
            &file,
            1024,
            4,
            131072,
            &control,
            Instant::now(),
            Duration::from_secs(1),
        )
        .unwrap();
        let existing = workspace.0.join("existing.json");
        fs::write(&existing, b"keep existing output").unwrap();
        let mut output = File::options().write(true).open(&existing).unwrap();
        assert!(write(
            &mut output,
            &manifest,
            1024,
            &control,
            Instant::now(),
            Duration::from_secs(1)
        )
        .is_err());
        assert_eq!(fs::read(&existing).unwrap(), b"keep existing output");
        let limited = workspace.0.join("limited.json");
        let mut output = File::create(&limited).unwrap();
        assert!(write(
            &mut output,
            &manifest,
            64,
            &control,
            Instant::now(),
            Duration::from_secs(1)
        )
        .unwrap_err()
        .contains("Dateigrößenlimit"));
        assert!(output.metadata().unwrap().len() <= 64);
        let invalid = workspace.0.join("invalid.json");
        let mut output = File::create(&invalid).unwrap();
        manifest.entries[1].sha256 = Some("A".repeat(64));
        assert!(write(
            &mut output,
            &manifest,
            1024,
            &control,
            Instant::now(),
            Duration::from_secs(1)
        )
        .is_err());
        assert_eq!(output.metadata().unwrap().len(), 0);
        manifest.entries[1].sha256 = Some("0".repeat(64));
        control.cancel();
        assert!(write(
            &mut output,
            &manifest,
            1024,
            &control,
            Instant::now(),
            Duration::from_secs(1)
        )
        .is_err());
        assert_eq!(output.metadata().unwrap().len(), 0);
    }
}
