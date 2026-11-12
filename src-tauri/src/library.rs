use rusqlite::{params, Connection, OpenFlags};
#[path = "assets.rs"]
mod assets;
pub use assets::AssetKind;
#[path = "model_resources.rs"]
mod model_resources;
pub use model_resources::{ModelResource, ModelResources};
#[path = "image_groups.rs"]
mod image_groups;
#[path = "sprite_export.rs"]
mod sprite_export;
pub use image_groups::{
    ImageGroup, ImageGroupMember, ImageGroupSummary, ImageWorkflow, KindCounts, MaterialRole,
};
pub(crate) use sprite_export::SpritePayload;
#[path = "audio.rs"]
mod audio;
#[cfg(any(feature = "desktop", test))]
#[path = "audio_tags.rs"]
mod audio_tags;
#[cfg(feature = "desktop")]
pub(crate) use audio_tags::EmbeddedAudioTags;
#[cfg(target_os = "macos")]
#[path = "volume.rs"]
mod volume;
pub use audio::{AudioMetadata, AudioStatus, AudioWaveform, WaveformPeak};
#[path = "audio_filters.rs"]
mod audio_filters;
pub(crate) use audio_filters::AudioCriteria;
use std::{
    collections::HashMap,
    fs, io,
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, String>;

#[path = "imports.rs"]
mod imports;
pub use imports::{ImportControl, ImportProgress, ZipEstimate};
use imports::{ImportRun, ImportedFile};
#[path = "bulk.rs"]
mod bulk;
pub use bulk::{BulkAction, BulkResult, BulkSelection};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPackage {
    pub id: i64,
    pub name: String,
    pub relative_path: String,
    pub source_path: String,
    pub asset_count: i64,
    pub source_kind: String,
    pub available: bool,
    pub external_root: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetFile {
    pub id: i64,
    pub original_filename: String,
    pub relative_path: String,
    pub original_folder: String,
    pub format: String,
    pub kind: AssetKind,
    pub size_bytes: i64,
    pub audio: Option<AudioMetadata>,
    pub available: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: i64,
    pub name: String,
    pub kind: AssetKind,
    pub package_id: i64,
    pub package_name: String,
    pub original_folder: String,
    pub files: Vec<AssetFile>,
    pub tags: Vec<String>,
    pub folder_ids: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_group: Option<ImageGroupSummary>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFolder {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub asset_count: i64,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogQuery {
    #[serde(default)]
    pub text: String,
    pub asset_id: Option<i64>,
    pub package_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub original_folder: Option<String>,
    pub format: Option<String>,
    pub kind: Option<AssetKind>,
    pub workflow: Option<ImageWorkflow>,
    pub tag: Option<String>,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFilters {
    pub folders: Vec<String>,
    pub formats: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPage {
    pub items: Vec<Asset>,
    pub total: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ExportOutcome {
    Cancelled,
    ReplacementRequired,
    Exported { path: String, size_bytes: u64 },
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    pub id: i64,
    pub package_id: i64,
    pub file_id: Option<i64>,
    pub name: String,
    pub package_name: String,
    pub deleted_at: i64,
    pub original_folder: Option<String>,
    pub parent_trashed: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptiedTrash {
    pub packages: i64,
    pub files: i64,
    pub disk_errors: Vec<String>,
}

pub struct Library {
    root: PathBuf,
    connection: Connection,
}

// File provenance is derived from the immutable package-relative path, even after regrouping.
const FILE_FOLDER: &str = "substr(f.relative_path, length(p.relative_path) + 2, max(0, length(f.relative_path) - length(p.relative_path) - length(f.original_filename) - 2))";

fn catalog_filter(query: &CatalogQuery) -> (&'static str, String) {
    let folder_filter = "WITH RECURSIVE selected_folders(id) AS (
        SELECT id FROM folders WHERE id = ?6 UNION
        SELECT f.id FROM folders f JOIN selected_folders s ON f.parent_id = s.id)";
    let kind_filter = query
        .kind
        .map(|kind| format!("AND f.format IN ('{}')", kind.formats().join("','")))
        .unwrap_or_default();
    let filter = format!("FROM assets a JOIN packages p ON p.id = a.package_id JOIN active_asset_files f ON f.asset_id = a.id
            WHERE (?1 IS NULL OR p.id = ?1)
            AND (?2 = '' OR instr(lower(a.name || ' ' || p.name || ' ' || {FILE_FOLDER} || ' ' || f.original_filename), lower(?2)) > 0
                OR EXISTS(SELECT 1 FROM asset_tags t WHERE t.asset_id = a.id AND instr(lower(t.tag), lower(?2)) > 0)
                OR EXISTS(SELECT 1 FROM package_tags t WHERE t.package_id = p.id AND instr(lower(t.tag), lower(?2)) > 0))
            AND (?3 IS NULL OR {FILE_FOLDER} = ?3)
            AND (?4 IS NULL OR f.format = upper(?4))
            AND (?5 IS NULL OR EXISTS(SELECT 1 FROM asset_tags t WHERE t.asset_id = a.id AND t.tag = ?5 COLLATE NOCASE)
                OR EXISTS(SELECT 1 FROM package_tags t WHERE t.package_id = p.id AND t.tag = ?5 COLLATE NOCASE))
            AND (?6 IS NULL OR EXISTS(SELECT 1 FROM folder_assets fa JOIN selected_folders s ON s.id = fa.folder_id WHERE fa.asset_id = a.id))
            AND (?7 IS NULL OR a.id = ?7) {kind_filter}");
    (folder_filter, filter)
}

impl Library {
    pub fn create(root: &Path) -> Result<Self> {
        if root.exists()
            && fs::read_dir(root)
                .map_err(|e| format!("Speicherort nicht lesbar: {e}"))?
                .next()
                .is_some()
        {
            return Err("Bitte einen leeren Ordner für die neue Bibliothek auswählen".into());
        }
        fs::create_dir_all(root)
            .map_err(|e| format!("Bibliotheksordner kann nicht erstellt werden: {e}"))?;
        let root = root
            .canonicalize()
            .map_err(|e| format!("Speicherort nicht verfügbar: {e}"))?;
        let connection = Connection::open(root.join("catalog.sqlite3"))
            .map_err(|e| format!("Katalog kann nicht erstellt werden: {e}"))?;
        connection.execute_batch("PRAGMA user_version = 1;
            CREATE TABLE packages (id INTEGER PRIMARY KEY, name TEXT NOT NULL, relative_path TEXT NOT NULL UNIQUE, source_path TEXT NOT NULL);
            CREATE TABLE assets (id INTEGER PRIMARY KEY, package_id INTEGER NOT NULL REFERENCES packages(id), name TEXT NOT NULL, original_folder TEXT NOT NULL);
            CREATE TABLE asset_files (id INTEGER PRIMARY KEY, asset_id INTEGER NOT NULL REFERENCES assets(id), original_filename TEXT NOT NULL, relative_path TEXT NOT NULL UNIQUE, format TEXT NOT NULL, size_bytes INTEGER NOT NULL);").map_err(|e| format!("Katalog kann nicht gespeichert werden: {e}"))?;
        drop(connection);
        Self::open(&root)
    }

    pub fn create_default(home: &Path) -> Result<Self> {
        let root = home.join("Game Asset Library");
        if root.join("catalog.sqlite3").exists() {
            Self::open(&root)
        } else {
            Self::create(&root)
        }
    }

    fn connect(root: &Path, flags: OpenFlags) -> Result<Self> {
        let root = root
            .canonicalize()
            .map_err(|e| format!("Bibliotheksordner nicht verfügbar: {e}"))?;
        let catalog = root.join("catalog.sqlite3");
        if fs::symlink_metadata(&catalog)
            .map_err(|e| format!("Kein Bibliothekskatalog gefunden: {e}"))?
            .file_type()
            .is_symlink()
        {
            return Err("Der Katalog darf keine symbolische Verknüpfung sein".into());
        }
        let connection = Connection::open_with_flags(catalog, flags)
            .map_err(|e| format!("Katalog kann nicht geöffnet werden: {e}"))?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| format!("Katalogformat nicht lesbar: {e}"))?;
        // Version 2 only marks that the one-time macOS metadata repair has run.
        if !(1..=2).contains(&version) {
            return Err(format!("Nicht unterstütztes Katalogformat: {version}"));
        }
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|e| format!("Katalog nicht verfügbar: {e}"))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|e| format!("Katalog nicht verfügbar: {e}"))?;
        Ok(Self { root, connection })
    }

    pub fn open_read_only(root: &Path) -> Result<Self> {
        let library = Self::connect(root, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        library
            .connection
            .pragma_update(None, "query_only", true)
            .map_err(|e| format!("Katalog nicht verfügbar: {e}"))?;
        library.connection.prepare("SELECT a.id FROM active_assets a LEFT JOIN folders f ON 0 LEFT JOIN folder_assets fa ON 0 LIMIT 0")
            .map_err(|_| "Bitte die Bibliothek zuerst in der App öffnen, um den Katalog zu aktualisieren")?;
        Ok(library)
    }

    /// Per-command connection to a catalog that `open` already prepared when it was activated.
    pub fn writable(root: &Path) -> Result<Self> {
        Self::connect(root, OpenFlags::SQLITE_OPEN_READ_WRITE)
    }

    /// Activates a library: schema, one-time repairs and crash leftovers. Not for per-command use.
    pub fn open(root: &Path) -> Result<Self> {
        let Self { root, connection } = Self::writable(root)?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS external_sources (package_id INTEGER PRIMARY KEY REFERENCES packages(id), root TEXT NOT NULL, identity TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS external_files (file_id INTEGER PRIMARY KEY REFERENCES asset_files(id), identity TEXT NOT NULL, modified TEXT NOT NULL);").map_err(|e| format!("Externe Quellen können nicht geöffnet werden: {e}"))?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS asset_tags (asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE, tag TEXT NOT NULL COLLATE NOCASE, PRIMARY KEY(asset_id, tag));
            CREATE TABLE IF NOT EXISTS package_tags (package_id INTEGER NOT NULL REFERENCES packages(id) ON DELETE CASCADE, tag TEXT NOT NULL COLLATE NOCASE, PRIMARY KEY(package_id, tag));
            CREATE TABLE IF NOT EXISTS audio_metadata (file_id INTEGER PRIMARY KEY REFERENCES asset_files(id) ON DELETE CASCADE, duration_seconds REAL, sample_rate INTEGER, channels INTEGER, codec TEXT, error TEXT);
            CREATE INDEX IF NOT EXISTS asset_files_asset_id ON asset_files(asset_id);")
            .map_err(|e| format!("Annotationen können nicht geöffnet werden: {e}"))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS trash (
            id INTEGER PRIMARY KEY,
            package_id INTEGER UNIQUE REFERENCES packages(id) ON DELETE CASCADE,
            file_id INTEGER UNIQUE REFERENCES asset_files(id) ON DELETE CASCADE,
            deleted_at INTEGER NOT NULL DEFAULT (unixepoch()),
            CHECK ((package_id IS NULL) != (file_id IS NULL)));
            CREATE VIEW IF NOT EXISTS active_packages AS SELECT p.* FROM packages p
                WHERE NOT EXISTS (SELECT 1 FROM trash t WHERE t.package_id = p.id);
            CREATE VIEW IF NOT EXISTS active_asset_files AS SELECT f.* FROM asset_files f
                JOIN assets a ON a.id = f.asset_id JOIN active_packages p ON p.id = a.package_id
                WHERE NOT EXISTS (SELECT 1 FROM trash t WHERE t.file_id = f.id);
            CREATE VIEW IF NOT EXISTS active_assets AS SELECT a.* FROM assets a
                WHERE EXISTS (SELECT 1 FROM active_asset_files f WHERE f.asset_id = a.id);",
            )
            .map_err(|e| format!("Papierkorb kann nicht geöffnet werden: {e}"))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS folders (
            id INTEGER PRIMARY KEY, name TEXT NOT NULL, parent_id INTEGER REFERENCES folders(id));
            CREATE INDEX IF NOT EXISTS folders_parent_id ON folders(parent_id);
            CREATE TABLE IF NOT EXISTS folder_assets (
                folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
                PRIMARY KEY(folder_id, asset_id));
            CREATE INDEX IF NOT EXISTS folder_assets_asset_id ON folder_assets(asset_id);",
            )
            .map_err(|e| format!("Projektordner können nicht geöffnet werden: {e}"))?;
        let mut library = Self { root, connection };
        library.remove_import_leftovers();
        let version: i64 = library
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| format!("Katalogformat nicht lesbar: {e}"))?;
        if version < 2 {
            library.remove_macos_metadata_records()?;
            library
                .connection
                .pragma_update(None, "user_version", 2)
                .map_err(|e| format!("Katalogformat kann nicht gespeichert werden: {e}"))?;
        }
        // Cheap when every source is migrated; offline legacy sources retry on the next activation.
        library.migrate_external_identities()?;
        Ok(library)
    }

    /// Staging folders of imports interrupted by a crash; never follows links.
    fn remove_import_leftovers(&self) {
        let packages = self.root.join("packages");
        if fs::symlink_metadata(&packages).map_or(true, |m| !m.is_dir()) {
            return;
        }
        for entry in fs::read_dir(packages).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(".import-")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    fn remove_macos_metadata_records(&mut self) -> Result<()> {
        let files = self
            .connection
            .prepare("SELECT id, asset_id, relative_path FROM asset_files")
            .map_err(|e| format!("Katalog nicht lesbar: {e}"))?
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("Katalog nicht lesbar: {e}"))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| format!("Katalog nicht lesbar: {e}"))?
            .into_iter()
            .filter(|(_, _, path)| assets::is_macos_metadata(Path::new(path)))
            .collect::<Vec<_>>();
        if files.is_empty() {
            return Ok(());
        }
        let backup = self.root.join(format!(
            "catalog-before-macos-metadata-{}.sqlite3",
            unique_suffix()
        ));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .map_err(|e| format!("Katalogsicherung kann nicht erstellt werden: {e}"))?;
        self.connection
            .execute("VACUUM INTO ?1", [path_text(&backup)?])
            .map_err(|e| {
                format!("Katalogsicherung fehlgeschlagen; Katalog bleibt unverändert: {e}")
            })?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|e| format!("Katalogkorrektur kann nicht gestartet werden: {e}"))?;
        for (file_id, asset_id, path) in files {
            transaction
                .execute("DELETE FROM external_files WHERE file_id = ?1 AND EXISTS (SELECT 1 FROM asset_files WHERE id = ?1 AND asset_id = ?2 AND relative_path = ?3)", params![file_id, asset_id, path])
                .map_err(|e| {
                    format!("Begleitdatei kann nicht aus dem Katalog entfernt werden: {e}")
                })?;
            // Another connection may already have repaired and reimported a package.
            // Match the original provenance as well as IDs, which SQLite can reuse.
            let removed = transaction
                .execute("DELETE FROM asset_files WHERE id = ?1 AND asset_id = ?2 AND relative_path = ?3", params![file_id, asset_id, path])
                .map_err(|e| {
                    format!("Begleitdatei kann nicht aus dem Katalog entfernt werden: {e}")
                })?;
            if removed > 0 {
                transaction.execute("DELETE FROM assets WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM asset_files WHERE asset_id = ?1)", [asset_id])
                    .map_err(|e| format!("Begleitdatei-Eintrag kann nicht entfernt werden: {e}"))?;
            }
        }
        transaction
            .commit()
            .map_err(|e| format!("Katalogkorrektur kann nicht gespeichert werden: {e}"))
    }

    fn migrate_external_identities(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let sources = self.connection.prepare("SELECT s.package_id, s.root, s.identity, p.relative_path FROM external_sources s JOIN packages p ON p.id = s.package_id WHERE s.identity NOT LIKE 'mac-volume:%'")
                .map_err(|e| format!("Externe Quellen nicht lesbar: {e}"))?
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))
                .map_err(|e| format!("Externe Quellen nicht lesbar: {e}"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| format!("Externe Quellen nicht lesbar: {e}"))?;
            for (package_id, root, identity, prefix) in sources {
                let root = Path::new(&root);
                let Ok(metadata) = fs::symlink_metadata(root) else {
                    continue;
                };
                if !metadata.is_dir()
                    || metadata.file_type().is_symlink()
                    || legacy_file_identity(root, &metadata)? != identity
                {
                    continue;
                }
                let files = self.connection.prepare("SELECT f.id, f.relative_path, e.identity, e.modified, f.size_bytes FROM asset_files f JOIN assets a ON a.id = f.asset_id JOIN external_files e ON e.file_id = f.id WHERE a.package_id = ?1")
                    .map_err(|e| format!("Quellkatalog nicht lesbar: {e}"))?
                    .query_map([package_id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, i64>(4)?)))
                    .map_err(|e| format!("Quellkatalog nicht lesbar: {e}"))?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| format!("Quellkatalog nicht lesbar: {e}"))?;
                let converted = (|| -> Result<_> {
                    let root_identity = file_identity(root, &metadata)?;
                    let mut updates = Vec::new();
                    for (file_id, path, file_identity_before, modified, size) in files {
                        let relative = Path::new(&path)
                            .strip_prefix(&prefix)
                            .map_err(|_| "Ungültiger Dateipfad im Katalog")?;
                        let (path, metadata) = resolve_external(
                            root,
                            &identity,
                            relative,
                            &file_identity_before,
                            &modified,
                            size,
                            legacy_file_identity,
                        )?;
                        updates.push((file_id, file_identity(&path, &metadata)?));
                    }
                    Ok((root_identity, updates))
                })();
                // An offline or changed legacy source stays intact and unavailable; never learn
                // an unverified replacement's UUID or partially upgrade one package's files.
                let Ok((root_identity, updates)) = converted else {
                    continue;
                };
                let transaction = self
                    .connection
                    .transaction()
                    .map_err(|e| format!("Quellidentität kann nicht aktualisiert werden: {e}"))?;
                transaction
                    .execute(
                        "UPDATE external_sources SET identity = ?1 WHERE package_id = ?2",
                        params![root_identity, package_id],
                    )
                    .map_err(|e| format!("Quellidentität kann nicht gespeichert werden: {e}"))?;
                for (file_id, identity) in updates {
                    transaction
                        .execute(
                            "UPDATE external_files SET identity = ?1 WHERE file_id = ?2",
                            params![identity, file_id],
                        )
                        .map_err(|e| {
                            format!("Dateiidentität kann nicht gespeichert werden: {e}")
                        })?;
                }
                transaction
                    .commit()
                    .map_err(|e| format!("Quellidentität kann nicht gespeichert werden: {e}"))?;
            }
        }
        Ok(())
    }

    fn save_import(
        &mut self,
        name: String,
        source_path: String,
        relative_path: String,
        files: Vec<ImportedFile>,
        run: &mut ImportRun<'_>,
    ) -> Result<AssetPackage> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|e| format!("Katalog kann nicht aktualisiert werden: {e}"))?;
        transaction
            .execute(
                "INSERT INTO packages(name, relative_path, source_path) VALUES (?1, ?2, ?3)",
                params![name, relative_path, source_path],
            )
            .map_err(|e| format!("Paket kann nicht katalogisiert werden: {e}"))?;
        let id = transaction.last_insert_rowid();
        let mut groups = HashMap::new();
        for file in &files {
            run.cataloging(&file.relative)?;
            insert_variant(&transaction, id, &relative_path, file, &mut groups)?;
        }
        run.check()?;
        transaction
            .commit()
            .map_err(|e| format!("Katalog kann nicht gespeichert werden: {e}"))?;
        Ok(AssetPackage {
            id,
            name,
            relative_path,
            source_path,
            asset_count: groups.len() as i64,
            source_kind: "managed".into(),
            available: true,
            external_root: None,
            tags: Vec::new(),
        })
    }

    pub fn reference_folder(&mut self, source: &Path) -> Result<AssetPackage> {
        let source = source
            .canonicalize()
            .map_err(|e| format!("Externe Quelle nicht verfügbar: {e}"))?;
        if !source.is_dir() || source.starts_with(&self.root) || self.root.starts_with(&source) {
            return Err("Bitte einen Quellordner außerhalb der Bibliothek auswählen".into());
        }
        let source_path = path_text(&source)?;
        let identity = file_identity(
            &source,
            &fs::metadata(&source).map_err(|e| format!("Externe Quelle nicht verfügbar: {e}"))?,
        )?;
        let name = path_text(
            source
                .file_name()
                .map(Path::new)
                .ok_or("Quelle hat keinen Namen")?,
        )?;
        let mut files = Vec::new();
        discover_external(&source, Path::new(""), &mut files)?;
        let relative_path = format!("external/{}", unique_suffix());
        let transaction = self
            .connection
            .transaction()
            .map_err(|e| format!("Katalog kann nicht aktualisiert werden: {e}"))?;
        transaction
            .execute(
                "INSERT INTO packages(name, relative_path, source_path) VALUES (?1, ?2, ?3)",
                params![name, relative_path, source_path],
            )
            .map_err(|e| format!("Quelle kann nicht katalogisiert werden: {e}"))?;
        let id = transaction.last_insert_rowid();
        transaction
            .execute(
                "INSERT INTO external_sources(package_id, root, identity) VALUES (?1, ?2, ?3)",
                params![id, source_path, identity],
            )
            .map_err(|e| format!("Quelle kann nicht gespeichert werden: {e}"))?;
        let mut groups = HashMap::new();
        for file in &files {
            let file_id = insert_variant(
                &transaction,
                id,
                &relative_path,
                &ImportedFile {
                    relative: file.relative.clone(),
                    format: file.format.clone(),
                    size: file.size,
                },
                &mut groups,
            )?;
            transaction
                .execute(
                    "INSERT INTO external_files(file_id, identity, modified) VALUES (?1, ?2, ?3)",
                    params![file_id, file.identity, file.modified],
                )
                .map_err(|e| format!("Dateiherkunft kann nicht gespeichert werden: {e}"))?;
        }
        transaction
            .commit()
            .map_err(|e| format!("Katalog kann nicht gespeichert werden: {e}"))?;
        Ok(AssetPackage {
            id,
            name,
            relative_path,
            external_root: Some(source_path.clone()),
            source_path,
            asset_count: groups.len() as i64,
            source_kind: "external".into(),
            available: true,
            tags: Vec::new(),
        })
    }

    pub fn relink_source(&mut self, package_id: i64, source: &Path) -> Result<AssetPackage> {
        let source = source
            .canonicalize()
            .map_err(|e| format!("Ersatzquelle nicht verfügbar: {e}"))?;
        if source.starts_with(&self.root) || self.root.starts_with(&source) {
            return Err("Externe Quelle und Bibliothek dürfen nicht ineinander liegen".into());
        }
        let (identity, prefix): (String, String) = self.connection.query_row("SELECT s.identity, p.relative_path FROM external_sources s JOIN active_packages p ON p.id = s.package_id WHERE s.package_id = ?1", [package_id], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|e| format!("Externe Quelle nicht im Katalog: {e}"))?;
        let metadata =
            fs::metadata(&source).map_err(|e| format!("Ersatzquelle nicht verfügbar: {e}"))?;
        // ponytail: filesystem identities support moves on the same volume; cross-volume copies need content verification before relinking.
        if !metadata.is_dir() || file_identity(&source, &metadata)? != identity {
            return Err("Dieser Ordner ist nicht die ursprüngliche externe Quelle. Wähle den verschobenen Originalordner; eine Kopie bitte als neue Quelle einbinden".into());
        }
        let mut statement = self.connection.prepare("SELECT f.relative_path, e.identity, e.modified, f.size_bytes FROM asset_files f JOIN assets a ON a.id = f.asset_id JOIN external_files e ON e.file_id = f.id WHERE a.package_id = ?1").map_err(|e| format!("Quellkatalog nicht lesbar: {e}"))?;
        let files = statement
            .query_map([package_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| format!("Quellkatalog nicht lesbar: {e}"))?;
        for file in files {
            let (path, stored_identity, modified, size) =
                file.map_err(|e| format!("Quellkatalog nicht lesbar: {e}"))?;
            let relative = Path::new(&path)
                .strip_prefix(&prefix)
                .map_err(|_| "Ungültiger Dateipfad im Katalog")?;
            resolve_external(
                &source,
                &identity,
                relative,
                &stored_identity,
                &modified,
                size,
                file_identity,
            )?;
        }
        drop(statement);
        self.connection
            .execute(
                "UPDATE external_sources SET root = ?1 WHERE package_id = ?2",
                params![path_text(&source)?, package_id],
            )
            .map_err(|e| format!("Quellpfad kann nicht gespeichert werden: {e}"))?;
        self.packages()?
            .into_iter()
            .find(|package| package.id == package_id)
            .ok_or_else(|| "Externe Quelle nicht im Katalog".into())
    }

    pub fn packages(&self) -> Result<Vec<AssetPackage>> {
        let mut statement = self.connection.prepare("SELECT p.id, p.name, p.relative_path, p.source_path, COUNT(a.id), s.root, s.identity FROM active_packages p LEFT JOIN active_assets a ON a.package_id = p.id LEFT JOIN external_sources s ON s.package_id = p.id GROUP BY p.id ORDER BY p.name COLLATE NOCASE, p.id").map_err(|e| format!("Pakete nicht lesbar: {e}"))?;
        let rows = statement
            .query_map([], |row| {
                let external_root: Option<String> = row.get(5)?;
                let identity: Option<String> = row.get(6)?;
                let available = external_root.as_ref().is_none_or(|root| {
                    fs::symlink_metadata(root).ok().is_some_and(|metadata| {
                        metadata.is_dir()
                            && !metadata.file_type().is_symlink()
                            && file_identity(Path::new(root), &metadata).ok().as_ref()
                                == identity.as_ref()
                    })
                });
                Ok(AssetPackage {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    relative_path: row.get(2)?,
                    source_path: row.get(3)?,
                    asset_count: row.get(4)?,
                    source_kind: if external_root.is_some() {
                        "external"
                    } else {
                        "managed"
                    }
                    .into(),
                    available,
                    external_root,
                    tags: Vec::new(),
                })
            })
            .map_err(|e| format!("Pakete nicht lesbar: {e}"))?;
        let mut packages = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| format!("Pakete nicht lesbar: {e}"))?;
        for package in &mut packages {
            package.tags = self.select_strings(
                "SELECT tag FROM package_tags WHERE package_id = ?1 ORDER BY tag COLLATE NOCASE",
                Some(package.id),
            )?;
        }
        Ok(packages)
    }

    pub fn folders(&self) -> Result<Vec<ProjectFolder>> {
        let mut statement = self
            .connection
            .prepare(
                "WITH RECURSIVE descendants(folder_id, child_id) AS (
            SELECT id, id FROM folders UNION
            SELECT d.folder_id, f.id FROM folders f JOIN descendants d ON f.parent_id = d.child_id)
            SELECT f.id, f.name, f.parent_id, COUNT(DISTINCT a.id) FROM folders f
            JOIN descendants d ON d.folder_id = f.id
            LEFT JOIN folder_assets fa ON fa.folder_id = d.child_id
            LEFT JOIN active_assets a ON a.id = fa.asset_id
            GROUP BY f.id ORDER BY f.name COLLATE NOCASE, f.id",
            )
            .map_err(|e| format!("Projektordner nicht lesbar: {e}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(ProjectFolder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    parent_id: row.get(2)?,
                    asset_count: row.get(3)?,
                })
            })
            .map_err(|e| format!("Projektordner nicht lesbar: {e}"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| format!("Projektordner nicht lesbar: {e}"))
    }

    pub fn create_folder(&mut self, name: &str, parent_id: Option<i64>) -> Result<ProjectFolder> {
        let trimmed = name.trim();
        if trimmed.is_empty()
            || matches!(trimmed, "." | "..")
            || trimmed.chars().count() > 256
            || name
                .chars()
                .any(|character| character.is_control() || character == '/' || character == '\\')
        {
            return Err("Der Ordnername muss 1 bis 256 Zeichen enthalten, ohne Steuerzeichen, Schrägstriche, . oder ..".into());
        }
        if let Some(parent_id) = parent_id {
            let exists: bool = self
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                    [parent_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Übergeordneter Projektordner nicht lesbar: {e}"))?;
            if !exists {
                return Err("Übergeordneter Projektordner nicht im Katalog".into());
            }
        }
        let name = trimmed;
        self.connection
            .execute(
                "INSERT INTO folders(name, parent_id) VALUES (?1, ?2)",
                params![name, parent_id],
            )
            .map_err(|e| format!("Projektordner kann nicht angelegt werden: {e}"))?;
        Ok(ProjectFolder {
            id: self.connection.last_insert_rowid(),
            name: name.into(),
            parent_id,
            asset_count: 0,
        })
    }

    pub fn add_to_folder(&mut self, folder_id: i64, asset_id: i64) -> Result<()> {
        self.validate_folder_asset(folder_id, asset_id)?;
        self.connection
            .execute(
                "INSERT OR IGNORE INTO folder_assets(folder_id, asset_id) VALUES (?1, ?2)",
                params![folder_id, asset_id],
            )
            .map_err(|e| format!("Sound kann nicht zum Projektordner hinzugefügt werden: {e}"))?;
        Ok(())
    }

    pub fn remove_from_folder(&mut self, folder_id: i64, asset_id: i64) -> Result<()> {
        self.validate_folder_asset(folder_id, asset_id)?;
        self.connection
            .execute(
                "DELETE FROM folder_assets WHERE folder_id = ?1 AND asset_id = ?2",
                params![folder_id, asset_id],
            )
            .map_err(|e| format!("Sound kann nicht aus dem Projektordner entfernt werden: {e}"))?;
        Ok(())
    }

    fn validate_folder_asset(&self, folder_id: i64, asset_id: i64) -> Result<()> {
        let valid: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)
            AND EXISTS(SELECT 1 FROM active_assets WHERE id = ?2)",
                params![folder_id, asset_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Ordnerzuordnung nicht lesbar: {e}"))?;
        if !valid {
            return Err("Projektordner oder Sound nicht im aktiven Katalog".into());
        }
        Ok(())
    }

    pub fn trash_package(&mut self, package_id: i64) -> Result<()> {
        let inserted = self
            .connection
            .execute(
                "INSERT INTO trash(package_id) SELECT id FROM active_packages WHERE id = ?1",
                [package_id],
            )
            .map_err(|e| format!("Paket kann nicht in den Papierkorb verschoben werden: {e}"))?;
        if inserted == 0 {
            return Err("Paket nicht im aktiven Katalog".into());
        }
        Ok(())
    }

    pub fn trash_file(&mut self, file_id: i64) -> Result<()> {
        let inserted = self
            .connection
            .execute(
                "INSERT INTO trash(file_id) SELECT id FROM active_asset_files WHERE id = ?1",
                [file_id],
            )
            .map_err(|e| format!("Datei kann nicht in den Papierkorb verschoben werden: {e}"))?;
        if inserted == 0 {
            return Err("Asset-Datei nicht im aktiven Katalog".into());
        }
        Ok(())
    }

    pub fn trash_entries(&self) -> Result<Vec<TrashEntry>> {
        let mut statement = self.connection.prepare(
            &format!("SELECT t.id, p.id, t.file_id, coalesce(f.original_filename, p.name), p.name, t.deleted_at,
                t.file_id IS NOT NULL AND EXISTS (SELECT 1 FROM trash parent WHERE parent.package_id = p.id),
                CASE WHEN t.file_id IS NOT NULL THEN {FILE_FOLDER} END
             FROM trash t LEFT JOIN asset_files f ON f.id = t.file_id LEFT JOIN assets a ON a.id = f.asset_id
                JOIN packages p ON p.id = coalesce(t.package_id, a.package_id)
             ORDER BY t.deleted_at DESC, t.id DESC"),
        ).map_err(|e| format!("Papierkorb nicht lesbar: {e}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(TrashEntry {
                    id: row.get(0)?,
                    package_id: row.get(1)?,
                    file_id: row.get(2)?,
                    name: row.get(3)?,
                    package_name: row.get(4)?,
                    deleted_at: row.get(5)?,
                    parent_trashed: row.get(6)?,
                    original_folder: row.get(7)?,
                })
            })
            .map_err(|e| format!("Papierkorb nicht lesbar: {e}"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| format!("Papierkorbeintrag nicht lesbar: {e}"))
    }

    pub fn restore_trash(&mut self, trash_id: i64) -> Result<()> {
        let restored = self.connection.execute(
            "DELETE FROM trash WHERE id = ?1 AND NOT EXISTS (
                SELECT 1 FROM asset_files f JOIN assets a ON a.id = f.asset_id
                    JOIN trash parent ON parent.package_id = a.package_id WHERE f.id = trash.file_id)",
            [trash_id],
        ).map_err(|e| format!("Eintrag kann nicht wiederhergestellt werden: {e}"))?;
        if restored == 0 {
            return Err(
                "Papierkorbeintrag nicht verfügbar. Stelle zuerst das zugehörige Paket wieder her."
                    .into(),
            );
        }
        Ok(())
    }

    pub fn empty_trash(&mut self) -> Result<EmptiedTrash> {
        let failed = |e: rusqlite::Error| format!("Papierkorb kann nicht geleert werden: {e}");
        let transaction = self.connection.transaction().map_err(failed)?;
        let (packages, files): (i64, i64) = transaction
            .query_row(
                "SELECT count(package_id), count(file_id) FROM trash",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(failed)?;
        // Only managed data lives under the library root; external packages lose catalog rows only.
        let managed: Vec<String> = transaction
            .prepare(
                "SELECT f.relative_path FROM trash t JOIN asset_files f ON f.id = t.file_id
                    JOIN assets a ON a.id = f.asset_id
                    WHERE NOT EXISTS (SELECT 1 FROM external_sources s WHERE s.package_id = a.package_id)
                    AND NOT EXISTS (SELECT 1 FROM external_files e WHERE e.file_id = f.id)
                 UNION ALL
                 SELECT p.relative_path FROM trash t JOIN packages p ON p.id = t.package_id
                    WHERE NOT EXISTS (SELECT 1 FROM external_sources s WHERE s.package_id = p.id)",
            )
            .map_err(failed)?
            .query_map([], |row| row.get(0))
            .map_err(failed)?
            .collect::<std::result::Result<_, _>>()
            .map_err(failed)?;
        transaction
            .execute_batch(
                "CREATE TEMP TABLE doomed_packages AS SELECT package_id AS id FROM trash WHERE package_id IS NOT NULL;
                CREATE TEMP TABLE doomed_files AS
                    SELECT f.id FROM asset_files f JOIN assets a ON a.id = f.asset_id WHERE a.package_id IN doomed_packages
                    UNION SELECT file_id FROM trash WHERE file_id IS NOT NULL;
                CREATE TEMP TABLE doomed_assets AS
                    SELECT asset_id AS id FROM asset_files WHERE id IN doomed_files
                    UNION SELECT id FROM assets WHERE package_id IN doomed_packages;
                DELETE FROM trash WHERE file_id IN doomed_files OR package_id IN doomed_packages;
                DELETE FROM external_files WHERE file_id IN doomed_files;
                DELETE FROM audio_metadata WHERE file_id IN doomed_files;
                DELETE FROM asset_files WHERE id IN doomed_files;
                DELETE FROM doomed_assets WHERE EXISTS (SELECT 1 FROM asset_files f WHERE f.asset_id = doomed_assets.id);
                DELETE FROM asset_tags WHERE asset_id IN doomed_assets;
                DELETE FROM folder_assets WHERE asset_id IN doomed_assets;
                DELETE FROM assets WHERE id IN doomed_assets;
                DELETE FROM package_tags WHERE package_id IN doomed_packages;
                DELETE FROM external_sources WHERE package_id IN doomed_packages;
                DELETE FROM packages WHERE id IN doomed_packages;
                DROP TABLE temp.doomed_packages; DROP TABLE temp.doomed_files; DROP TABLE temp.doomed_assets;",
            )
            .map_err(failed)?;
        transaction.commit().map_err(failed)?;
        let disk_errors = managed
            .iter()
            .filter_map(|relative| {
                remove_managed(&self.root, relative)
                    .err()
                    .map(|e| format!("{relative}: {e}"))
            })
            .collect();
        Ok(EmptiedTrash {
            packages,
            files,
            disk_errors,
        })
    }

    pub fn annotate_asset(&mut self, asset_id: i64, name: &str, tags: &[String]) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 256 {
            return Err("Der Asset-Name muss 1 bis 256 Zeichen enthalten".into());
        }
        let tags = normalized_tags(tags)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|e| format!("Annotationen können nicht gespeichert werden: {e}"))?;
        if transaction
            .execute(
                "UPDATE assets SET name = ?1 WHERE id = ?2 AND id IN (SELECT id FROM active_assets)",
                params![name, asset_id],
            )
            .map_err(|e| format!("Asset kann nicht aktualisiert werden: {e}"))?
            == 0
        {
            return Err("Asset nicht im Katalog".into());
        }
        transaction
            .execute("DELETE FROM asset_tags WHERE asset_id = ?1", [asset_id])
            .map_err(|e| format!("Tags können nicht aktualisiert werden: {e}"))?;
        for tag in tags {
            transaction
                .execute(
                    "INSERT INTO asset_tags(asset_id, tag) VALUES (?1, ?2)",
                    params![asset_id, tag],
                )
                .map_err(|e| format!("Tag kann nicht gespeichert werden: {e}"))?;
        }
        transaction
            .commit()
            .map_err(|e| format!("Annotationen können nicht gespeichert werden: {e}"))
    }

    pub fn annotate_package(&mut self, package_id: i64, tags: &[String]) -> Result<()> {
        let tags = normalized_tags(tags)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|e| format!("Annotationen können nicht gespeichert werden: {e}"))?;
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM active_packages WHERE id = ?1)",
                [package_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Paket nicht lesbar: {e}"))?;
        if !exists {
            return Err("Paket nicht im Katalog".into());
        }
        transaction
            .execute(
                "DELETE FROM package_tags WHERE package_id = ?1",
                [package_id],
            )
            .map_err(|e| format!("Tags können nicht aktualisiert werden: {e}"))?;
        for tag in tags {
            transaction
                .execute(
                    "INSERT INTO package_tags(package_id, tag) VALUES (?1, ?2)",
                    params![package_id, tag],
                )
                .map_err(|e| format!("Tag kann nicht gespeichert werden: {e}"))?;
        }
        transaction
            .commit()
            .map_err(|e| format!("Annotationen können nicht gespeichert werden: {e}"))
    }

    pub fn split_variant(&mut self, file_id: i64) -> Result<i64> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|e| format!("Variantenzuordnung kann nicht gespeichert werden: {e}"))?;
        let (asset_id, package_id, name, folder, count): (i64, i64, String, String, i64) = transaction.query_row(
            &format!("SELECT a.id, a.package_id, a.name, {FILE_FOLDER}, (SELECT COUNT(*) FROM active_asset_files WHERE asset_id = a.id) FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id WHERE f.id = ?1"),
            [file_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ).map_err(|e| format!("Asset-Datei nicht im Katalog: {e}"))?;
        if count < 2 {
            return Err("Diese Datei ist bereits ein eigenes Asset".into());
        }
        transaction
            .execute(
                "INSERT INTO assets(package_id, name, original_folder) VALUES (?1, ?2, ?3)",
                params![package_id, name, folder],
            )
            .map_err(|e| format!("Asset kann nicht angelegt werden: {e}"))?;
        let new_id = transaction.last_insert_rowid();
        transaction.execute("INSERT INTO asset_tags(asset_id, tag) SELECT ?1, tag FROM asset_tags WHERE asset_id = ?2", params![new_id, asset_id])
            .map_err(|e| format!("Tags können nicht übernommen werden: {e}"))?;
        transaction.execute("INSERT INTO folder_assets(folder_id, asset_id) SELECT folder_id, ?1 FROM folder_assets WHERE asset_id = ?2", params![new_id, asset_id])
            .map_err(|e| format!("Projektordner können nicht übernommen werden: {e}"))?;
        transaction
            .execute(
                "UPDATE asset_files SET asset_id = ?1 WHERE id = ?2",
                params![new_id, file_id],
            )
            .map_err(|e| format!("Datei kann nicht zugeordnet werden: {e}"))?;
        transaction
            .commit()
            .map_err(|e| format!("Variantenzuordnung kann nicht gespeichert werden: {e}"))?;
        Ok(new_id)
    }

    pub fn assign_variant(&mut self, file_id: i64, target_asset_id: i64) -> Result<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|e| format!("Variantenzuordnung kann nicht gespeichert werden: {e}"))?;
        // Retained trashed variants still need their original asset and annotations after restoration.
        let (source_id, source_package, source_name, filename, count, source_format): (i64, i64, String, String, i64, String) = transaction.query_row(
            "SELECT a.id, a.package_id, a.name, f.original_filename, (SELECT COUNT(*) FROM asset_files WHERE asset_id = a.id), f.format FROM active_asset_files f JOIN assets a ON a.id = f.asset_id WHERE f.id = ?1",
            [file_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        ).map_err(|e| format!("Asset-Datei nicht im Katalog: {e}"))?;
        let (target_package, target_name, target_folder, target_format): (i64, String, String, String) = transaction
            .query_row(
                "SELECT a.package_id, a.name, a.original_folder, f.format FROM active_assets a JOIN active_asset_files f ON f.asset_id = a.id WHERE a.id = ?1 LIMIT 1",
                [target_asset_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|e| format!("Ziel-Asset nicht im Katalog: {e}"))?;
        if source_id == target_asset_id {
            return Err("Die Datei gehört bereits zu diesem Asset".into());
        }
        if source_package != target_package {
            return Err("Varianten müssen im selben Paket bleiben; ihre Originalpfade werden nicht verschoben".into());
        }
        let source_kind =
            AssetKind::for_format(&source_format).ok_or("Nicht unterstütztes Asset-Format")?;
        let target_kind =
            AssetKind::for_format(&target_format).ok_or("Nicht unterstütztes Asset-Format")?;
        if source_kind != target_kind {
            return Err(
                "Varianten unterschiedlicher Asset-Typen können nicht zusammengeführt werden"
                    .into(),
            );
        }
        let original_name = Path::new(&filename)
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or("Datei hat keinen Namen")?;
        if count == 1 && source_name != original_name && source_name != target_name {
            let target_is_original = {
                let mut statement = transaction.prepare(&format!("SELECT f.original_filename, {FILE_FOLDER} FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id WHERE a.id = ?1"))
                    .map_err(|e| format!("Zielherkunft nicht lesbar: {e}"))?;
                let origins = statement
                    .query_map([target_asset_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| format!("Zielherkunft nicht lesbar: {e}"))?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| format!("Zielherkunft nicht lesbar: {e}"))?;
                origins.iter().any(|(filename, folder)| {
                    folder == &target_folder
                        && Path::new(filename)
                            .file_stem()
                            .and_then(|name| name.to_str())
                            == Some(target_name.as_str())
                })
            };
            if !target_is_original {
                return Err(format!("Die letzte Datei trägt den gespeicherten Namen „{source_name}“, das Ziel heißt „{target_name}“. Benenne beide Assets gleich, wenn sie zusammengehören. Namen und Tags wurden nicht geändert."));
            }
            transaction
                .execute(
                    "UPDATE assets SET name = ?1 WHERE id = ?2",
                    params![source_name, target_asset_id],
                )
                .map_err(|e| format!("Asset-Name kann nicht übernommen werden: {e}"))?;
        }
        let merged_tags = {
            let mut statement = transaction.prepare("SELECT tag FROM asset_tags WHERE asset_id = ?1 OR asset_id = ?2 ORDER BY asset_id = ?2 DESC, tag COLLATE NOCASE")
                .map_err(|e| format!("Tags nicht lesbar: {e}"))?;
            let tags = statement
                .query_map(params![source_id, target_asset_id], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| format!("Tags nicht lesbar: {e}"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| format!("Tags nicht lesbar: {e}"))?;
            normalized_tags(&tags)?
        };
        for tag in merged_tags {
            transaction
                .execute(
                    "INSERT OR IGNORE INTO asset_tags(asset_id, tag) VALUES (?1, ?2)",
                    params![target_asset_id, tag],
                )
                .map_err(|e| format!("Tags können nicht übernommen werden: {e}"))?;
        }
        transaction.execute("INSERT OR IGNORE INTO folder_assets(folder_id, asset_id) SELECT folder_id, ?1 FROM folder_assets WHERE asset_id = ?2", params![target_asset_id, source_id])
            .map_err(|e| format!("Projektordner können nicht übernommen werden: {e}"))?;
        transaction
            .execute(
                "UPDATE asset_files SET asset_id = ?1 WHERE id = ?2",
                params![target_asset_id, file_id],
            )
            .map_err(|e| format!("Datei kann nicht zugeordnet werden: {e}"))?;
        if count == 1 {
            transaction
                .execute("DELETE FROM assets WHERE id = ?1", [source_id])
                .map_err(|e| format!("Bisherige Zuordnung kann nicht abgeschlossen werden: {e}"))?;
        }
        transaction
            .commit()
            .map_err(|e| format!("Variantenzuordnung kann nicht gespeichert werden: {e}"))
    }

    fn select_strings(&self, sql: &str, id: Option<i64>) -> Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare(sql)
            .map_err(|e| format!("Katalogwerte nicht lesbar: {e}"))?;
        let rows = statement
            .query_map([id], |row| row.get(0))
            .map_err(|e| format!("Katalogwerte nicht lesbar: {e}"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| format!("Katalogwerte nicht lesbar: {e}"))
    }

    pub fn catalog_filters(&self, package_id: Option<i64>) -> Result<CatalogFilters> {
        Ok(CatalogFilters {
            folders: self.select_strings(&format!("SELECT DISTINCT {FILE_FOLDER} AS original_folder FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id WHERE (?1 IS NULL OR p.id = ?1) ORDER BY original_folder COLLATE NOCASE"), package_id)?,
            formats: self.select_strings("SELECT DISTINCT f.format FROM active_asset_files f JOIN assets a ON a.id = f.asset_id WHERE (?1 IS NULL OR a.package_id = ?1) ORDER BY f.format", package_id)?,
            tags: self.select_strings("SELECT t.tag FROM asset_tags t JOIN active_assets a ON a.id = t.asset_id WHERE (?1 IS NULL OR a.package_id = ?1) UNION SELECT t.tag FROM package_tags t JOIN active_packages p ON p.id = t.package_id WHERE (?1 IS NULL OR t.package_id = ?1) ORDER BY tag COLLATE NOCASE", package_id)?,
        })
    }

    pub(crate) fn prepare_audio_analysis(
        &self,
        query: CatalogQuery,
        audio: Option<AudioCriteria>,
    ) -> Result<Vec<i64>> {
        if query.kind != Some(AssetKind::Sound) || query.workflow.is_some() {
            return Err("Bitte einen Sound-Suchbereich ohne Bild-Workflow auswählen".into());
        }
        let audio = audio.unwrap_or_default();
        audio.validate(&query)?;
        let (folders, mut filter) = catalog_filter(&query);
        filter.push_str(audio_filters::FILTER);
        let mut statement = self
            .connection
            .prepare(&format!(
                "{folders} SELECT DISTINCT f.id {filter} ORDER BY f.id LIMIT ?8"
            ))
            .map_err(|error| format!("Sound-Suchbereich nicht lesbar: {error}"))?;
        let ids = statement
            .query_map(
                params![
                    query.package_id,
                    query.text.trim(),
                    query.original_folder,
                    query.format,
                    query.tag,
                    query.folder_id,
                    query.asset_id,
                    100_001,
                    0,
                    audio.min_duration_seconds,
                    audio.max_duration_seconds,
                    audio.channels.as_deref().unwrap_or("any"),
                    audio.analysis.as_deref().unwrap_or("any"),
                ],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?
            .collect::<std::result::Result<Vec<i64>, _>>()
            .map_err(|error| error.to_string())?;
        // ponytail: one explicit serial job is bounded to 100,000 variants; split larger libraries by package.
        if ids.len() > 100_000 {
            return Err("Der Suchbereich enthält mehr als 100.000 Sound-Dateien. Bitte ein Paket oder einen engeren Suchbereich auswählen".into());
        }
        Ok(ids)
    }

    pub fn query(&self, query: CatalogQuery) -> Result<CatalogPage> {
        self.query_with_audio(query, None)
    }

    pub(crate) fn query_with_audio(
        &self,
        mut query: CatalogQuery,
        audio: Option<AudioCriteria>,
    ) -> Result<CatalogPage> {
        let audio = audio.unwrap_or_default();
        let active_audio = audio.validate(&query)?;
        if active_audio {
            query.kind = Some(AssetKind::Sound);
        }
        if query.workflow.is_some() {
            return self.query_image_groups(query);
        }
        let limit = if query.limit == 0 {
            40
        } else {
            query.limit.clamp(1, 100)
        };
        let snapshot = if active_audio {
            Some(
                self.connection
                    .unchecked_transaction()
                    .map_err(|e| format!("Katalogsuche fehlgeschlagen: {e}"))?,
            )
        } else {
            None
        };
        let (folder_filter, mut filter) = catalog_filter(&query);
        filter.push_str(audio_filters::FILTER);
        let total = self
            .connection
            .query_row(
                &format!("{folder_filter} SELECT COUNT(DISTINCT a.id) {filter}"),
                params![
                    query.package_id,
                    query.text.trim(),
                    query.original_folder,
                    query.format,
                    query.tag,
                    query.folder_id,
                    query.asset_id,
                    limit,
                    0,
                    audio.min_duration_seconds,
                    audio.max_duration_seconds,
                    audio.channels.as_deref().unwrap_or("any"),
                    audio.analysis.as_deref().unwrap_or("any")
                ],
                |row| row.get(0),
            )
            .map_err(|e| format!("Katalogsuche fehlgeschlagen: {e}"))?;
        let offset = if query.offset >= total {
            0
        } else {
            query.offset.max(0)
        };
        let sql = format!("{folder_filter} SELECT a.id, a.name, p.id, p.name, a.original_folder, MIN(f.format), MIN(f.id) {filter} GROUP BY a.id ORDER BY a.name COLLATE NOCASE, p.id, a.original_folder, a.id LIMIT ?8 OFFSET ?9");
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|e| format!("Katalogsuche fehlgeschlagen: {e}"))?;
        let rows = statement
            .query_map(
                params![
                    query.package_id,
                    query.text.trim(),
                    query.original_folder,
                    query.format,
                    query.tag,
                    query.folder_id,
                    query.asset_id,
                    limit,
                    offset,
                    audio.min_duration_seconds,
                    audio.max_duration_seconds,
                    audio.channels.as_deref().unwrap_or("any"),
                    audio.analysis.as_deref().unwrap_or("any")
                ],
                |row| Ok((asset_row(row)?, row.get::<_, i64>(6)?)),
            )
            .map_err(|e| format!("Katalogsuche fehlgeschlagen: {e}"))?;
        let (mut items, matching): (Vec<_>, Vec<_>) = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| format!("Katalogeintrag nicht lesbar: {e}"))?
            .into_iter()
            .unzip();
        drop(statement);
        self.hydrate(&mut items)?;
        if active_audio {
            for (asset, matching_file_id) in items.iter_mut().zip(matching) {
                if let Some(index) = asset
                    .files
                    .iter()
                    .position(|file| file.id == matching_file_id)
                {
                    let representative = asset.files.remove(index);
                    asset.files.insert(0, representative);
                }
            }
        }
        if let Some(snapshot) = snapshot {
            snapshot
                .commit()
                .map_err(|e| format!("Katalogsuche fehlgeschlagen: {e}"))?;
        }
        Ok(CatalogPage {
            items,
            total,
            offset,
            limit,
        })
    }

    /// Active catalog rows for the given asset ids, in id order, without files, tags or folders.
    pub(super) fn assets_by_id(&self, ids: &[i64]) -> Result<Vec<Asset>> {
        self.connection
            .prepare("SELECT a.id, a.name, p.id, p.name, a.original_folder, MIN(f.format) FROM assets a JOIN packages p ON p.id = a.package_id JOIN active_asset_files f ON f.asset_id = a.id WHERE a.id IN (SELECT value FROM json_each(?1)) GROUP BY a.id ORDER BY a.id")
            .and_then(|mut statement| {
                statement
                    .query_map([id_list(ids)], asset_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|e| format!("Katalogeintrag nicht lesbar: {e}"))
    }

    /// Loads tags, project folders and files of a whole page with one query each.
    pub(super) fn hydrate(&self, assets: &mut [Asset]) -> Result<()> {
        let ids = assets.iter().map(|asset| asset.id).collect::<Vec<_>>();
        let mut files = self.files_for_assets(&ids)?;
        let mut tags = self.by_asset::<String>("SELECT asset_id, tag FROM asset_tags WHERE asset_id IN (SELECT value FROM json_each(?1)) ORDER BY tag COLLATE NOCASE", &ids)?;
        let mut folders = self.by_asset::<i64>("SELECT asset_id, folder_id FROM folder_assets WHERE asset_id IN (SELECT value FROM json_each(?1)) ORDER BY folder_id", &ids)?;
        for asset in assets.iter_mut() {
            asset.files = files.remove(&asset.id).unwrap_or_default();
            asset.tags = tags.remove(&asset.id).unwrap_or_default();
            asset.folder_ids = folders.remove(&asset.id).unwrap_or_default();
        }
        Ok(())
    }

    fn by_asset<T: rusqlite::types::FromSql>(
        &self,
        sql: &str,
        ids: &[i64],
    ) -> Result<HashMap<i64, Vec<T>>> {
        let mut grouped = HashMap::<i64, Vec<T>>::new();
        let mut statement = self
            .connection
            .prepare(sql)
            .map_err(|e| format!("Katalogeintrag nicht lesbar: {e}"))?;
        let rows = statement
            .query_map([id_list(ids)], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Katalogeintrag nicht lesbar: {e}"))?;
        for row in rows {
            let (asset_id, value) = row.map_err(|e| format!("Katalogeintrag nicht lesbar: {e}"))?;
            grouped.entry(asset_id).or_default().push(value);
        }
        Ok(grouped)
    }

    /// Active files grouped by asset, with audio metadata and availability, in one query.
    pub(super) fn files_for_assets(&self, ids: &[i64]) -> Result<HashMap<i64, Vec<AssetFile>>> {
        let mut statement = self.connection.prepare(&format!("SELECT f.asset_id, f.id, f.original_filename, f.relative_path, f.format, f.size_bytes, {FILE_FOLDER},
                s.root, s.identity, p.relative_path, e.identity, e.modified, f.size_bytes,
                m.file_id, m.duration_seconds, m.sample_rate, m.channels, m.codec, m.error
            FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id
            LEFT JOIN external_sources s ON s.package_id = p.id LEFT JOIN external_files e ON e.file_id = f.id
            LEFT JOIN audio_metadata m ON m.file_id = f.id
            WHERE a.id IN (SELECT value FROM json_each(?1)) ORDER BY f.format, f.original_filename, f.id"))
            .map_err(|e| format!("Asset-Dateien nicht lesbar: {e}"))?;
        let rows = statement
            .query_map([id_list(ids)], |row| {
                let format: String = row.get(4)?;
                let relative_path: String = row.get(3)?;
                let external = location_row(row, 7)?;
                let audio = match row.get::<_, Option<i64>>(13)? {
                    Some(_) => Some(audio_row(row, 14)?),
                    None => None,
                };
                Ok((
                    row.get::<_, i64>(0)?,
                    AssetFile {
                        id: row.get(1)?,
                        original_filename: row.get(2)?,
                        available: self.locate(&relative_path, external).is_ok(),
                        relative_path,
                        kind: AssetKind::for_format(&format)
                            .ok_or(rusqlite::Error::InvalidQuery)?,
                        format,
                        size_bytes: row.get(5)?,
                        original_folder: row.get(6)?,
                        audio,
                    },
                ))
            })
            .map_err(|e| format!("Asset-Dateien nicht lesbar: {e}"))?;
        let mut files = HashMap::<i64, Vec<AssetFile>>::new();
        for row in rows {
            let (asset_id, file) = row.map_err(|e| format!("Asset-Dateien nicht lesbar: {e}"))?;
            files.entry(asset_id).or_default().push(file);
        }
        Ok(files)
    }

    pub fn resolve_preview(&self, file_id: i64) -> Result<PathBuf> {
        let (relative, external): (String, Option<ReferencedLocation>) = self
            .connection
            .query_row(
                "SELECT f.relative_path, s.root, s.identity, p.relative_path, e.identity, e.modified, f.size_bytes FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id LEFT JOIN external_sources s ON s.package_id = p.id LEFT JOIN external_files e ON e.file_id = f.id WHERE f.id = ?1",
                [file_id],
                |row| Ok((row.get(0)?, location_row(row, 1)?)),
            )
            .map_err(|e| format!("Asset-Datei nicht im Katalog: {e}"))?;
        self.locate(&relative, external)
    }

    fn locate(&self, relative: &str, external: Option<ReferencedLocation>) -> Result<PathBuf> {
        if let Some(external) = external {
            let relative = Path::new(&relative)
                .strip_prefix(&external.prefix)
                .map_err(|_| "Ungültiger Dateipfad im Katalog")?;
            return resolve_external(
                Path::new(&external.root),
                &external.root_identity,
                relative,
                &external.identity,
                &external.modified,
                external.size,
                file_identity,
            )
            .map(|(path, _)| path);
        }
        let relative = Path::new(&relative);
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            || !relative.starts_with("packages")
        {
            return Err("Ungültiger Dateipfad im Katalog".into());
        }
        let mut candidate = self.root.clone();
        for part in relative.components() {
            candidate.push(part);
            if fs::symlink_metadata(&candidate)
                .map_err(|e| format!("Asset-Datei nicht verfügbar: {e}"))?
                .file_type()
                .is_symlink()
            {
                return Err("Asset-Datei wurde durch eine symbolische Verknüpfung ersetzt".into());
            }
        }
        if !candidate.is_file() {
            return Err("Asset-Datei nicht verfügbar".into());
        }
        Ok(candidate)
    }

    pub(crate) fn audio_analysis_busy_timeout(&self) -> Result<()> {
        self.connection
            .busy_timeout(Duration::from_millis(100))
            .map_err(|e| e.to_string())
    }

    pub fn analyze_audio(&self, file_id: i64) -> Result<AudioMetadata> {
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let result = self.inspect_audio(file_id, &cancelled)?;
        self.store_audio_inspection(file_id, result, &cancelled)
    }

    pub fn export(
        &self,
        file_id: i64,
        destination: Option<&Path>,
        overwrite: bool,
    ) -> Result<ExportOutcome> {
        let Some(destination) = destination else {
            self.connection
                .query_row(
                    "SELECT id FROM active_asset_files WHERE id = ?1",
                    [file_id],
                    |_| Ok(()),
                )
                .map_err(|e| format!("Asset-Datei nicht im aktiven Katalog: {e}"))?;
            return Ok(ExportOutcome::Cancelled);
        };
        let source = self.resolve_preview(file_id)?;
        let parent = destination
            .parent()
            .ok_or("Exportziel hat keinen Ordner")?
            .canonicalize()
            .map_err(|e| format!("Exportordner nicht verfügbar: {e}"))?;
        let filename = destination
            .file_name()
            .ok_or("Exportziel hat keinen Dateinamen")?;
        let destination = parent.join(filename);
        self.validate_export_parent(&parent)?;
        if let Ok(metadata) = fs::symlink_metadata(&destination) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("Exportziel muss eine reguläre Datei sein".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let source_metadata = fs::metadata(&source)
                    .map_err(|e| format!("Quelldatei nicht verfügbar: {e}"))?;
                if metadata.dev() == source_metadata.dev()
                    && metadata.ino() == source_metadata.ino()
                {
                    return Err("Exportziel ist dieselbe Datei wie die Bibliotheksquelle".into());
                }
            }
            if !overwrite {
                return Ok(ExportOutcome::ReplacementRequired);
            }
        }
        let temporary = parent.join(format!(".game-assets-export-{}", unique_suffix()));
        let result = (|| {
            let mut input =
                fs::File::open(&source).map_err(|e| format!("Quelldatei nicht lesbar: {e}"))?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| format!("Export kann nicht vorbereitet werden: {e}"))?;
            let size_bytes = io::copy(&mut input, &mut output)
                .map_err(|e| format!("Export konnte nicht kopiert werden: {e}"))?;
            output
                .sync_all()
                .map_err(|e| format!("Export konnte nicht gespeichert werden: {e}"))?;
            drop(output);
            if overwrite {
                fs::rename(&temporary, &destination)
                    .map_err(|e| format!("Exportziel konnte nicht ersetzt werden: {e}"))?;
            } else if let Err(error) = fs::hard_link(&temporary, &destination) {
                // Linking never replaces; only file systems without hard links copy a second time.
                if error.kind() == io::ErrorKind::AlreadyExists {
                    return Ok(ExportOutcome::ReplacementRequired);
                }
                let mut target = match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                {
                    Ok(target) => target,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        return Ok(ExportOutcome::ReplacementRequired)
                    }
                    Err(error) => {
                        return Err(format!("Exportziel kann nicht erstellt werden: {error}"))
                    }
                };
                let write_result = (|| {
                    let mut input = fs::File::open(&temporary)?;
                    io::copy(&mut input, &mut target)?;
                    target.sync_all()
                })();
                if let Err(error) = write_result {
                    drop(target);
                    let _ = fs::remove_file(&destination);
                    return Err(format!("Export konnte nicht gespeichert werden: {error}"));
                }
            }
            Ok(ExportOutcome::Exported {
                path: path_text(&destination)?,
                size_bytes,
            })
        })();
        let _ = fs::remove_file(&temporary);
        result
    }

    fn validate_export_parent(&self, parent: &Path) -> Result<()> {
        if parent.starts_with(&self.root) {
            return Err("Exportziel darf die Bibliothek nicht verändern".into());
        }
        if self
            .external_roots()?
            .iter()
            .any(|root| parent.starts_with(root))
        {
            return Err("Exportziel darf die externe Quelle nicht verändern".into());
        }
        Ok(())
    }

    /// Roots of all referenced sources, including trashed packages that may be restored.
    pub fn external_roots(&self) -> Result<Vec<PathBuf>> {
        self.connection
            .prepare("SELECT root FROM external_sources")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))?
                    .collect()
            })
            .map_err(|e| format!("Externe Quellpfade nicht lesbar: {e}"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn normalized_tags(tags: &[String]) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for tag in tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
    {
        if tag.chars().count() > 64 {
            return Err("Ein Tag darf höchstens 64 Zeichen enthalten".into());
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.to_lowercase() == tag.to_lowercase())
        {
            normalized.push(tag.to_owned());
        }
    }
    if normalized.len() > 32 {
        return Err("Bitte höchstens 32 Tags pro Asset oder Paket vergeben".into());
    }
    Ok(normalized)
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Dateiname ist nicht als Unicode lesbar".into())
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

struct ReferencedFile {
    relative: PathBuf,
    size: i64,
    identity: String,
    modified: String,
    format: String,
}

fn id_list(ids: &[i64]) -> String {
    serde_json::Value::from(ids).to_string()
}

fn asset_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Asset> {
    Ok(Asset {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: AssetKind::for_format(&row.get::<_, String>(5)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        package_id: row.get(2)?,
        package_name: row.get(3)?,
        original_folder: row.get(4)?,
        tags: Vec::new(),
        folder_ids: Vec::new(),
        files: Vec::new(),
        image_group: None,
    })
}

/// Columns: root, root identity, package prefix, file identity, modified, size.
fn location_row(
    row: &rusqlite::Row<'_>,
    at: usize,
) -> rusqlite::Result<Option<ReferencedLocation>> {
    let Some(root) = row.get::<_, Option<String>>(at)? else {
        return Ok(None);
    };
    Ok(Some(ReferencedLocation {
        root,
        root_identity: row.get(at + 1)?,
        prefix: row.get(at + 2)?,
        identity: row.get(at + 3)?,
        modified: row.get(at + 4)?,
        size: row.get(at + 5)?,
    }))
}

/// Columns: duration, sample rate, channels, codec, error.
fn audio_row(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<AudioMetadata> {
    let error: Option<String> = row.get(at + 4)?;
    Ok(AudioMetadata {
        status: if error.is_some() {
            AudioStatus::Error
        } else {
            AudioStatus::Ready
        },
        duration_seconds: row.get(at)?,
        sample_rate: row.get(at + 1)?,
        channels: row.get(at + 2)?,
        codec: row.get(at + 3)?,
        error,
    })
}

struct ReferencedLocation {
    root: String,
    root_identity: String,
    prefix: String,
    identity: String,
    modified: String,
    size: i64,
}

fn file_identity(path: &Path, metadata: &fs::Metadata) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "mac-volume:{}:{}",
            volume::uuid(path)?,
            metadata.ino()
        ))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = path;
        use std::os::unix::fs::MetadataExt;
        Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, metadata);
        Err("Externe Quellen werden auf diesem Betriebssystem noch nicht unterstützt".into())
    }
}

#[cfg(target_os = "macos")]
fn legacy_file_identity(_path: &Path, metadata: &fs::Metadata) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

fn modified_identity(metadata: &fs::Metadata) -> Result<String> {
    Ok(format!(
        "{:?}",
        metadata
            .modified()
            .map_err(|e| format!("Dateiänderung nicht lesbar: {e}"))?
    ))
}

fn discover_external(root: &Path, relative: &Path, files: &mut Vec<ReferencedFile>) -> Result<()> {
    for entry in fs::read_dir(root.join(relative))
        .map_err(|e| format!("Externer Quellordner nicht lesbar: {e}"))?
    {
        let entry = entry.map_err(|e| format!("Externe Quelle nicht lesbar: {e}"))?;
        let kind = entry
            .file_type()
            .map_err(|e| format!("Dateityp nicht lesbar: {e}"))?;
        let path = relative.join(entry.file_name());
        if kind.is_symlink() {
            return Err(format!(
                "Symbolische Verknüpfung wird nicht eingebunden: {}",
                path.display()
            ));
        }
        if kind.is_dir() {
            discover_external(root, &path, files)?;
        } else if kind.is_file() {
            let Some(format) = assets::candidate_format(&path) else {
                continue;
            };
            let metadata = entry
                .metadata()
                .map_err(|e| format!("Dateimetadaten nicht lesbar: {e}"))?;
            files.push(ReferencedFile {
                relative: path,
                size: i64::try_from(metadata.len())
                    .map_err(|_| "Datei ist zu groß für den Katalog")?,
                identity: file_identity(&entry.path(), &metadata)?,
                modified: modified_identity(&metadata)?,
                format: format.into(),
            });
        } else if !kind.is_file() {
            return Err(format!(
                "Nur reguläre Dateien werden eingebunden: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn insert_variant(
    transaction: &Connection,
    package_id: i64,
    prefix: &str,
    file: &ImportedFile,
    groups: &mut HashMap<(String, String, AssetKind), i64>,
) -> Result<i64> {
    let folder = path_text(file.relative.parent().unwrap_or(Path::new("")))?;
    let filename = file
        .relative
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Datei hat keinen Namen")?;
    let name = file
        .relative
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("Datei hat keinen Namen")?;
    let kind = AssetKind::for_format(&file.format).ok_or("Nicht unterstütztes Asset-Format")?;
    let asset_id = match groups.entry((folder.clone(), name.to_owned(), kind)) {
        std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            transaction
                .execute(
                    "INSERT INTO assets(package_id, name, original_folder) VALUES (?1, ?2, ?3)",
                    params![package_id, name, folder],
                )
                .map_err(|e| format!("Asset kann nicht katalogisiert werden: {e}"))?;
            *entry.insert(transaction.last_insert_rowid())
        }
    };
    let path = format!("{prefix}/{}", path_text(&file.relative)?);
    transaction.execute("INSERT INTO asset_files(asset_id, original_filename, relative_path, format, size_bytes) VALUES (?1, ?2, ?3, ?4, ?5)", params![asset_id, filename, path, file.format, file.size])
        .map_err(|e| format!("Asset-Datei kann nicht katalogisiert werden: {e}"))?;
    Ok(transaction.last_insert_rowid())
}

/// Removes a managed `packages/...` path below `root`; refuses symlinks anywhere on the way.
fn remove_managed(root: &Path, relative: &str) -> Result<()> {
    let relative = Path::new(relative);
    let mut parts = relative.components();
    if parts.next() != Some(Component::Normal("packages".as_ref()))
        || parts.clone().next().is_none()
        || !parts.all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("Ungültiger Pfad im Katalog".into());
    }
    let mut path = root.to_owned();
    let mut metadata = None;
    for part in relative.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("Symbolische Verknüpfung wird nicht gelöscht".into())
            }
            Ok(m) => metadata = Some(m),
            // Already gone, e.g. a trashed file inside a trashed package that was removed first.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
    }
    let removed = match metadata {
        Some(m) if m.is_dir() => fs::remove_dir_all(&path),
        _ => fs::remove_file(&path),
    };
    match removed {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

fn resolve_external(
    root: &Path,
    root_identity: &str,
    relative: &Path,
    identity: &str,
    modified: &str,
    size: i64,
    identify: fn(&Path, &fs::Metadata) -> Result<String>,
) -> Result<(PathBuf, fs::Metadata)> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Ungültiger Dateipfad im Katalog".into());
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|e| format!("Externe Quelle nicht verfügbar. Bitte Quelle neu verknüpfen: {e}"))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || identify(root, &metadata)? != root_identity
    {
        return Err(
            "Externe Quelle wurde ersetzt. Bitte den ursprünglichen Ordner neu verknüpfen".into(),
        );
    }
    let mut candidate = root.to_owned();
    for part in relative.components() {
        candidate.push(part);
        if fs::symlink_metadata(&candidate)
            .map_err(|e| format!("Externe Asset-Datei nicht verfügbar: {e}"))?
            .file_type()
            .is_symlink()
        {
            return Err(
                "Externe Asset-Datei wurde durch eine symbolische Verknüpfung ersetzt".into(),
            );
        }
    }
    let metadata = fs::metadata(&candidate)
        .map_err(|e| format!("Externe Asset-Datei nicht verfügbar: {e}"))?;
    if !metadata.is_file()
        || identify(&candidate, &metadata)? != identity
        || modified_identity(&metadata)? != modified
        || i64::try_from(metadata.len()).ok() != Some(size)
    {
        return Err("Externe Asset-Datei wurde geändert oder ersetzt. Bitte das geänderte Paket als neue Quelle einbinden".into());
    }
    Ok((candidate, metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "game-asset-workflow-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn material_workflow_browses_original_representatives_without_hiding_images() {
        let workspace = Workspace::new();
        let source = workspace.path("Textures");
        fs::create_dir_all(source.join("Stone")).unwrap();
        for name in [
            "Rock_Base_Color.png",
            "Rock_Normal.png",
            "Rock_Roughness.png",
            "Logo.png",
        ] {
            fs::write(source.join("Stone").join(name), name.as_bytes()).unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let originals = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(originals.total, 4);
        let original = originals
            .items
            .iter()
            .find(|asset| asset.name == "Rock_Base_Color")
            .unwrap();
        let grouped = library
            .query(
                serde_json::from_value(serde_json::json!({
                    "workflow": "material", "offset": 0, "limit": 40
                }))
                .unwrap(),
            )
            .unwrap();
        assert_eq!(grouped.total, 1);
        assert_eq!(grouped.items[0].id, original.id);
        assert_eq!(grouped.items[0].files[0].id, original.files[0].id);
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 4);
    }

    #[test]
    fn kind_counts_match_catalog_totals_per_type() {
        let workspace = Workspace::new();
        let source = workspace.path("Mixed");
        fs::create_dir_all(&source).unwrap();
        for name in [
            "Rock_Base_Color.png",
            "Rock_Normal.png",
            "Walk_01.png",
            "Walk_02.png",
            "Hit.wav",
            "Hit.mp3",
            "Crate.obj",
        ] {
            fs::write(source.join(name), name.as_bytes()).unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let counts = library.kind_counts().unwrap();
        for (kind, workflow, expected) in [
            (Some(AssetKind::Sound), None, counts.sound),
            (Some(AssetKind::Image), None, counts.image),
            (Some(AssetKind::Model), None, counts.model),
            (
                Some(AssetKind::Image),
                Some(ImageWorkflow::Material),
                counts.material,
            ),
            (
                Some(AssetKind::Image),
                Some(ImageWorkflow::SpriteSequence),
                counts.sprite_sequence,
            ),
        ] {
            let total = library
                .query(CatalogQuery {
                    kind,
                    workflow,
                    ..Default::default()
                })
                .unwrap()
                .total;
            assert_eq!(total, expected, "{kind:?} {workflow:?}");
        }
        assert_eq!(
            (
                counts.sound,
                counts.image,
                counts.model,
                counts.material,
                counts.sprite_sequence
            ),
            (1, 4, 1, 1, 1)
        );
    }

    #[test]
    fn material_workflow_exposes_duplicate_roles_in_one_source_folder() {
        let workspace = Workspace::new();
        let source = workspace.path("Textures");
        fs::create_dir_all(source.join("Stone")).unwrap();
        for name in [
            "Stone_Base_Color.png",
            "Stone_basecolor.png",
            "Stone_Normal.png",
        ] {
            fs::write(source.join("Stone").join(name), name.as_bytes()).unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let page = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        let group = page.items[0].image_group.as_ref().unwrap();
        assert_eq!(group.name, "Stone");
        assert_eq!(group.file_count, 3);
        assert_eq!(group.errors.len(), 1);
        assert!(group.errors[0].contains("baseColor"));
    }

    #[test]
    fn grouped_material_filters_match_members_before_paging_and_keep_sources_separate() {
        let workspace = Workspace::new();
        let source = workspace.path("Textures");
        fs::create_dir_all(source.join("maps")).unwrap();
        for index in 0..45 {
            for role in ["Base_Color", "Normal"] {
                let name = format!("Rock{index:02}_{role}.png");
                fs::write(source.join("maps").join(&name), name.as_bytes()).unwrap();
            }
        }
        fs::create_dir_all(source.join("other")).unwrap();
        fs::write(
            source.join("other/Rock00_Base_Color.png"),
            b"separate folder",
        )
        .unwrap();
        let other = workspace.path("Other package");
        fs::create_dir_all(other.join("maps")).unwrap();
        fs::write(
            other.join("maps/Rock00_Base_Color.png"),
            b"separate package",
        )
        .unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let package = library.import_folder(&source).unwrap();
        library.import_folder(&other).unwrap();
        let first = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                limit: 40,
                ..Default::default()
            })
            .unwrap();
        let last = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                limit: 40,
                offset: 40,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first.total, 47);
        assert_eq!(first.items.len(), 40);
        assert_eq!(last.items.len(), 7);
        assert_eq!(
            last.items
                .last()
                .unwrap()
                .image_group
                .as_ref()
                .unwrap()
                .name,
            "Rock44"
        );
        assert!(!first
            .items
            .iter()
            .any(|asset| last.items.iter().any(|other| asset.id == other.id)));
        let normal = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                text: "Rock03_Normal".into(),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        library
            .annotate_asset(normal.id, &normal.name, &["project".into()])
            .unwrap();
        let parent = library.create_folder("Game", None).unwrap();
        let child = library.create_folder("Materials", Some(parent.id)).unwrap();
        library.add_to_folder(child.id, normal.id).unwrap();
        let filtered = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                package_id: Some(package.id),
                folder_id: Some(parent.id),
                original_folder: Some("maps".into()),
                format: Some("png".into()),
                tag: Some("PROJECT".into()),
                text: "Normal".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.total, 1);
        assert_eq!(
            filtered.items[0].files[0].original_filename,
            "Rock03_Base_Color.png"
        );
        assert_eq!(
            filtered.items[0].image_group.as_ref().unwrap().file_count,
            2
        );
        assert_eq!(
            library
                .query(CatalogQuery {
                    workflow: Some(ImageWorkflow::Material),
                    kind: Some(AssetKind::Sound),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
    }

    #[test]
    fn material_group_snapshots_export_only_selected_original_maps_without_replacement() {
        let workspace = Workspace::new();
        let source = workspace.path("Textures");
        fs::create_dir_all(source.join("maps")).unwrap();
        for name in ["Stone_Base_Color.png", "Stone_Normal.png", "Logo.png"] {
            fs::write(source.join("maps").join(name), name.as_bytes()).unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let representative = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        let group = library
            .image_group(representative.files[0].id, ImageWorkflow::Material)
            .unwrap();
        assert_eq!(group.name, "Stone");
        assert_eq!(group.members.len(), 2);
        assert_eq!(group.resources.resources.len(), 2);
        assert!(group.errors.is_empty());
        assert!(group.resources.errors.is_empty());
        let normal = group
            .members
            .iter()
            .find(|member| member.file.original_filename == "Stone_Normal.png")
            .unwrap();
        assert_eq!(serde_json::to_value(normal.role).unwrap(), "normal");
        assert_eq!(normal.resource_path, "maps/Stone_Normal.png");
        let destination = workspace.path("Export");
        assert!(matches!(
            library
                .export_model(
                    &group.resources,
                    std::slice::from_ref(&normal.resource_path),
                    Some(&destination)
                )
                .unwrap(),
            ExportOutcome::Exported { .. }
        ));
        assert_eq!(
            fs::read(destination.join("maps/Stone_Normal.png")).unwrap(),
            b"Stone_Normal.png"
        );
        assert!(!destination.join("maps/Stone_Base_Color.png").exists());
        assert!(!destination.join("maps/Logo.png").exists());
        assert!(library
            .export_model(
                &group.resources,
                std::slice::from_ref(&normal.resource_path),
                Some(&destination)
            )
            .is_err());
        assert_eq!(
            fs::read(destination.join("maps/Stone_Normal.png")).unwrap(),
            b"Stone_Normal.png"
        );
        assert!(library
            .export_model(&group.resources, &[], Some(&workspace.path("Empty export")))
            .is_err());
        assert!(!workspace.path("Empty export").exists());
    }

    #[test]
    fn sprite_workflow_groups_only_same_folder_prefix_and_orders_numeric_frames() {
        let workspace = Workspace::new();
        let source = workspace.path("Sprites");
        fs::create_dir_all(source.join("water")).unwrap();
        for name in [
            "water10.png",
            "water2.png",
            "water1.png",
            "Logo.png",
            "single1.png",
        ] {
            fs::write(source.join("water").join(name), name.as_bytes()).unwrap();
        }
        fs::create_dir_all(source.join("other")).unwrap();
        fs::write(source.join("other/water3.png"), b"different folder").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let page = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::SpriteSequence),
                text: "water10".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].files[0].original_filename, "water1.png");
        let group = library
            .image_group(page.items[0].files[0].id, ImageWorkflow::SpriteSequence)
            .unwrap();
        assert_eq!(group.name, "water");
        assert_eq!(
            group
                .members
                .iter()
                .map(|member| member.frame_index.unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 10]
        );
        assert_eq!(
            group
                .members
                .iter()
                .map(|member| member.file.original_filename.as_str())
                .collect::<Vec<_>>(),
            ["water1.png", "water2.png", "water10.png"]
        );
        assert!(group.members.iter().all(|member| member.role.is_none()));
        assert!(group.errors.is_empty());
        let destination = workspace.path("Sequence export");
        let paths = group
            .members
            .iter()
            .map(|member| member.resource_path.clone())
            .collect::<Vec<_>>();
        library
            .export_model(&group.resources, &paths, Some(&destination))
            .unwrap();
        assert_eq!(
            fs::read(destination.join("water/water10.png")).unwrap(),
            b"water10.png"
        );
        assert!(!destination.join("other/water3.png").exists());
    }

    #[test]
    fn sprite_workflow_reports_duplicate_frame_indices_instead_of_picking_a_file() {
        let workspace = Workspace::new();
        let source = workspace.path("Sprites");
        fs::create_dir(&source).unwrap();
        for name in ["Same1.png", "Same01.png", "Same2.png"] {
            fs::write(source.join(name), name.as_bytes()).unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let page = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::SpriteSequence),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        let summary = page.items[0].image_group.as_ref().unwrap();
        assert_eq!(summary.file_count, 3);
        assert_eq!(summary.errors.len(), 1);
        let group = library
            .image_group(page.items[0].files[0].id, ImageWorkflow::SpriteSequence)
            .unwrap();
        assert_eq!(
            group
                .members
                .iter()
                .map(|member| member.frame_index.unwrap())
                .collect::<Vec<_>>(),
            [1, 1, 2]
        );
        assert_eq!(group.errors.len(), 1);
        assert!(group.errors[0].contains("1"));
        assert!(group.resources.errors.is_empty());
    }

    #[test]
    fn image_group_export_rechecks_trash_and_keeps_unrelated_sources_out() {
        let workspace = Workspace::new();
        let source = workspace.path("Textures");
        fs::create_dir(&source).unwrap();
        for name in [
            "Stone_Base_Color.png",
            "Stone_Normal.png",
            "Wood_Normal.png",
        ] {
            fs::write(source.join(name), name.as_bytes()).unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let package = library.import_folder(&source).unwrap();
        let representative = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                text: "Stone".into(),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        let group = library
            .image_group(representative.files[0].id, ImageWorkflow::Material)
            .unwrap();
        let normal = group
            .members
            .iter()
            .find(|member| member.role == Some(MaterialRole::Normal))
            .unwrap();
        assert!(library
            .export_model(
                &group.resources,
                &["Wood_Normal.png".into()],
                Some(&workspace.path("Wrong group"))
            )
            .is_err());
        assert!(!workspace.path("Wrong group").exists());
        library.trash_file(normal.file.id).unwrap();
        assert!(library
            .export_model(
                &group.resources,
                std::slice::from_ref(&normal.resource_path),
                Some(&workspace.path("Trashed"))
            )
            .is_err());
        assert!(!workspace.path("Trashed").exists());
        let active = library
            .image_group(representative.files[0].id, ImageWorkflow::Material)
            .unwrap();
        assert_eq!(active.members.len(), 1);
        assert_eq!(active.resources.resources.len(), 1);
        assert!(library.resolve_preview(normal.file.id).is_err());
        library.trash_package(package.id).unwrap();
        assert!(library
            .image_group(representative.files[0].id, ImageWorkflow::Material)
            .is_err());
        assert!(library
            .export_model(
                &active.resources,
                &[active.members[0].resource_path.clone()],
                Some(&workspace.path("Trashed package"))
            )
            .is_err());
        assert!(!workspace.path("Trashed package").exists());
    }

    #[cfg(unix)]
    #[test]
    fn image_group_external_changes_are_explicit_and_stale_exports_preserve_destinations() {
        let workspace = Workspace::new();
        let source = workspace.path("Textures");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Stone_Base_Color.png"), b"original color").unwrap();
        fs::write(source.join("Stone_Normal.png"), b"original normal").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        library.reference_folder(&source).unwrap();
        let representative = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        let group = library
            .image_group(representative.files[0].id, ImageWorkflow::Material)
            .unwrap();
        let paths = group
            .members
            .iter()
            .map(|member| member.resource_path.clone())
            .collect::<Vec<_>>();
        let exports = workspace.path("Exports");
        fs::create_dir(&exports).unwrap();
        assert!(library
            .export_model(&group.resources, &paths, Some(&source.join("Forbidden")))
            .is_err());
        assert!(!source.join("Forbidden").exists());
        fs::write(
            source.join("Stone_Normal.png"),
            b"changed original normal with a different length",
        )
        .unwrap();
        assert!(library
            .export_model(&group.resources, &paths, Some(&exports.join("Stale")))
            .is_err());
        assert!(!exports.join("Stale").exists());
        drop(library);
        let readonly = Library::open_read_only(&root).unwrap();
        let changed = readonly
            .image_group(representative.files[0].id, ImageWorkflow::Material)
            .unwrap();
        assert!(changed.errors.is_empty());
        assert_eq!(changed.resources.errors.len(), 1);
        assert!(changed.resources.errors[0].contains("Stone_Normal.png"));
        assert!(
            !changed
                .members
                .iter()
                .find(|member| member.role == Some(MaterialRole::Normal))
                .unwrap()
                .file
                .available
        );
        assert!(readonly
            .export_model(
                &changed.resources,
                &["Stone_Normal.png".into()],
                Some(&exports.join("Changed"))
            )
            .is_err());
        assert!(!exports.join("Changed").exists());
        let selected = exports.join("Selected color");
        assert!(matches!(
            readonly
                .export_model(
                    &changed.resources,
                    &["Stone_Base_Color.png".into()],
                    Some(&selected)
                )
                .unwrap(),
            ExportOutcome::Exported { .. }
        ));
        assert_eq!(
            fs::read(selected.join("Stone_Base_Color.png")).unwrap(),
            b"original color"
        );
        assert!(!selected.join("Stone_Normal.png").exists());
        assert_eq!(
            fs::read(source.join("Stone_Base_Color.png")).unwrap(),
            b"original color"
        );
    }

    #[test]
    fn image_group_export_rejects_a_changed_representative_even_when_it_is_omitted() {
        let workspace = Workspace::new();
        let source = workspace.path("Textures");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Stone_Base_Color.png"), b"color").unwrap();
        fs::write(source.join("Stone_Normal.png"), b"normal").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let representative = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        let group = library
            .image_group(representative.files[0].id, ImageWorkflow::Material)
            .unwrap();
        fs::write(
            library.resolve_preview(group.resources.file_id).unwrap(),
            b"changed representative color",
        )
        .unwrap();
        let destination = workspace.path("Stale selection");
        assert!(library
            .export_model(
                &group.resources,
                &["Stone_Normal.png".into()],
                Some(&destination)
            )
            .is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn grouped_files_share_annotations_but_remain_separately_addressable_after_regrouping() {
        let workspace = Workspace::new();
        let source = workspace.path("Textures");
        fs::create_dir_all(source.join("stone")).unwrap();
        fs::create_dir_all(source.join("wood")).unwrap();
        fs::write(source.join("stone/Stone_Base_Color.png"), b"stone map").unwrap();
        fs::write(source.join("wood/Wood_Base_Color.png"), b"wood map").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let originals = library.query(CatalogQuery::default()).unwrap();
        let stone = originals
            .items
            .iter()
            .find(|asset| asset.name == "Stone_Base_Color")
            .unwrap();
        let wood = originals
            .items
            .iter()
            .find(|asset| asset.name == "Wood_Base_Color")
            .unwrap();
        library.assign_variant(wood.files[0].id, stone.id).unwrap();
        let grouped = library
            .query(CatalogQuery {
                workflow: Some(ImageWorkflow::Material),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(grouped.total, 2);
        assert!(grouped.items.iter().all(|asset| asset.id == stone.id));
        let serialized = serde_json::to_value(&grouped).unwrap();
        assert_eq!(
            serialized["items"][0]["imageGroup"]["fileId"],
            stone.files[0].id
        );
        assert_eq!(
            serialized["items"][1]["imageGroup"]["fileId"],
            wood.files[0].id
        );
        assert_eq!(grouped.items[0].original_folder, "stone");
        assert_eq!(grouped.items[1].original_folder, "wood");
        for asset in &grouped.items {
            let group = library
                .image_group(asset.files[0].id, ImageWorkflow::Material)
                .unwrap();
            assert_eq!(group.members.len(), 1);
            assert_eq!(group.members[0].file.id, asset.files[0].id);
            assert_eq!(group.members[0].asset_id, stone.id);
        }
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 1);
    }

    #[test]
    fn mixed_imports_discover_visual_formats_without_merging_kinds_or_changing_bytes() {
        use std::io::Write;
        let workspace = Workspace::new();
        let source = workspace.path("Mixed package");
        let sound = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        let contents: &[(&str, &[u8])] = &[
            ("Shared/Beacon.mp3", sound),
            ("Shared/Beacon.png", b"original png"),
            ("Shared/Beacon.jpg", b"original jpg"),
            ("Shared/Beacon.JPEG", b"original jpeg"),
            ("Shared/Beacon.WebP", b"original webp"),
            (
                "Shared/Beacon.pSd",
                b"8BPS original layered photoshop source",
            ),
            ("Shared/Beacon.glb", b"original glb"),
            ("Shared/Beacon.gltf", b"original gltf"),
            ("Shared/Beacon.FBX", b"original fbx"),
            ("Shared/Beacon.obj", b"original obj"),
            ("Other/Icon.png", b"original icon"),
            ("Shared/._Beacon.png", b"AppleDouble metadata"),
            ("Shared/._Beacon.psd", b"AppleDouble PSD metadata"),
            ("__MACOSX/Shared/Ghost.glb", b"archive metadata"),
            ("Buffers/Beacon.bin", b"original buffer"),
            ("Shared/Beacon.mtl", b"original material"),
            ("LICENSE.txt", b"original license"),
        ];
        let archive = workspace.path("Mixed.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for (relative, bytes) in contents {
            let path = source.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
            zip.start_file(*relative, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let archive_bytes = fs::read(&archive).unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let packages = [
            library.import_folder(&source).unwrap(),
            library.import_zip(&archive).unwrap(),
            library.reference_folder(&source).unwrap(),
        ];
        for package in &packages {
            assert_eq!(package.asset_count, 4, "{}", package.source_kind);
            let page = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(page.total, 4);
            assert_eq!(
                page.items
                    .iter()
                    .filter(|asset| asset.name == "Beacon")
                    .count(),
                3
            );
            for (format, expected) in [("MP3", 1), ("PNG", 5), ("GLB", 4)] {
                let beacon = page
                    .items
                    .iter()
                    .find(|asset| {
                        asset.name == "Beacon"
                            && asset.files.iter().any(|file| file.format == format)
                    })
                    .unwrap();
                assert_eq!(beacon.original_folder, "Shared");
                assert_eq!(beacon.files.len(), expected);
                assert!(beacon.files.iter().all(|file| file.audio.is_none()));
            }
            let mut formats = library.catalog_filters(Some(package.id)).unwrap().formats;
            formats.sort();
            assert_eq!(
                formats,
                ["FBX", "GLB", "GLTF", "JPEG", "MP3", "OBJ", "PNG", "PSD", "WEBP"]
            );
            let package_root = if package.source_kind == "external" {
                source.clone()
            } else {
                root.join(&package.relative_path)
            };
            for (relative, bytes) in contents {
                assert_eq!(fs::read(package_root.join(relative)).unwrap(), *bytes);
            }
            for asset in page.items {
                for file in asset.files {
                    assert!(library.resolve_preview(file.id).unwrap().is_file());
                    if file.format == "PSD" {
                        assert_eq!(file.kind, AssetKind::Image);
                        let export = workspace.path(&format!("original-{}.psd", package.id));
                        assert!(matches!(
                            library.export(file.id, Some(&export), false).unwrap(),
                            ExportOutcome::Exported { .. }
                        ));
                        assert_eq!(
                            fs::read(&export).unwrap(),
                            b"8BPS original layered photoshop source"
                        );
                        assert!(matches!(
                            library.export(file.id, Some(&export), false).unwrap(),
                            ExportOutcome::ReplacementRequired
                        ));
                    }
                }
            }
        }
        assert_eq!(fs::read(archive).unwrap(), archive_bytes);
        drop(library);
        assert_eq!(
            Library::open_read_only(&root)
                .unwrap()
                .query(CatalogQuery::default())
                .unwrap()
                .total,
            12
        );
    }

    #[test]
    fn gif_imports_preserve_complete_animations_variants_and_model_exports() {
        use std::io::Write;
        let workspace = Workspace::new();
        let source = workspace.path("GIF source");
        let root = workspace.path("library");
        let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\x00\x00\x00\x00\x00\x21\xf9\x04\x00\x00\x00\x00\x00\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b".to_vec();
        let originals = std::collections::BTreeMap::from([
            ("Previews/Animation9.gif", gif.clone()),
            ("Previews/Animation55.GIF", gif.clone()),
            ("Textures/Checker.gif", gif.clone()),
            ("Textures/Checker.png", b"owned alternative".to_vec()),
            ("Textures/Stone_Base_Color.gif", gif.clone()),
            (
                "Model.obj",
                b"mtllib Model.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Owned\nf 1 2 3\n".to_vec(),
            ),
            (
                "Model.mtl",
                b"newmtl Owned\nmap_Kd Textures/Checker.gif\n".to_vec(),
            ),
        ]);
        let archive = workspace.path("GIF source.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for (name, bytes) in &originals {
            fs::create_dir_all(source.join(Path::new(name).parent().unwrap())).unwrap();
            fs::write(source.join(name), bytes).unwrap();
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let archive_bytes = fs::read(&archive).unwrap();
        let mut library = Library::create(&root).unwrap();
        let packages = [
            library.import_folder(&source).unwrap(),
            library.reference_folder(&source).unwrap(),
            library.import_zip(&archive).unwrap(),
        ];
        drop(library);
        let mut library = Library::open(&root).unwrap();
        for package in packages {
            let images = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    kind: Some(AssetKind::Image),
                    format: Some("gif".into()),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(images.total, 4);
            assert!(images
                .items
                .iter()
                .all(|asset| asset.files.iter().any(|file| file.format == "GIF")));
            assert_eq!(
                library
                    .query(CatalogQuery {
                        package_id: Some(package.id),
                        workflow: Some(ImageWorkflow::SpriteSequence),
                        ..Default::default()
                    })
                    .unwrap()
                    .total,
                0
            );
            let checker = images
                .items
                .iter()
                .find(|asset| asset.name == "Checker")
                .unwrap();
            assert_eq!(checker.files.len(), 2);
            let file = checker
                .files
                .iter()
                .find(|file| file.format == "GIF")
                .unwrap();
            assert_eq!(
                fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
                gif
            );
            let destination = workspace.path(&format!("Checker-{}.gif", package.id));
            library.export(file.id, Some(&destination), false).unwrap();
            assert_eq!(fs::read(destination).unwrap(), gif);
            let models = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    kind: Some(AssetKind::Model),
                    ..Default::default()
                })
                .unwrap();
            let snapshot = library
                .model_resources(models.items[0].files[0].id)
                .unwrap();
            assert!(snapshot
                .resources
                .iter()
                .any(|resource| resource.relative_path == "Textures/Checker.gif"));
            let paths = vec!["Model.mtl".into(), "Textures/Checker.gif".into()];
            let destination = workspace.path(&format!("Model-{}", package.id));
            library
                .export_model(&snapshot, &paths, Some(&destination))
                .unwrap();
            for name in ["Model.obj", "Model.mtl", "Textures/Checker.gif"] {
                assert_eq!(fs::read(destination.join(name)).unwrap(), originals[name]);
            }
            let materials = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    workflow: Some(ImageWorkflow::Material),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(materials.total, 1);
            library.trash_file(file.id).unwrap();
            assert!(library.resolve_preview(file.id).is_err());
            assert!(library
                .export_model(
                    &snapshot,
                    &paths,
                    Some(&workspace.path(&format!("Trashed-{}", package.id)))
                )
                .is_err());
        }
        for (name, bytes) in originals {
            assert_eq!(fs::read(source.join(name)).unwrap(), bytes);
        }
        assert_eq!(fs::read(archive).unwrap(), archive_bytes);
    }

    #[test]
    fn tga_import_is_searchable_after_reopen_and_exports_original_bytes() {
        let workspace = Workspace::new();
        let source = workspace.path("TGA source");
        let root = workspace.path("library");
        fs::create_dir(&source).unwrap();
        let original = [
            0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 32, 40, 0, 0, 255, 255,
        ];
        fs::write(source.join("Checker.tga"), original).unwrap();
        let mut library = Library::create(&root).unwrap();
        let package = library.import_folder(&source).unwrap();
        assert_eq!(package.asset_count, 1);
        drop(library);
        let library = Library::open(&root).unwrap();
        let page = library
            .query(CatalogQuery {
                kind: Some(AssetKind::Image),
                format: Some("tga".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].files[0].format, "TGA");
        let file = &page.items[0].files[0];
        assert_eq!(
            fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
            original
        );
        let exported = workspace.path("Game checker.tga");
        library.export(file.id, Some(&exported), false).unwrap();
        assert_eq!(fs::read(exported).unwrap(), original);
        assert_eq!(fs::read(source.join("Checker.tga")).unwrap(), original);
    }

    #[test]
    fn tga_folder_zip_and_reference_preserve_variants_groups_and_model_companions() {
        use std::io::Write;
        let workspace = Workspace::new();
        let source = workspace.path("TGA workflows");
        let root = workspace.path("library");
        let tga = vec![
            0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 32, 40, 0, 0, 255, 255,
        ];
        let originals = std::collections::BTreeMap::from([
            ("Textures/Checker.tga", tga.clone()),
            (
                "Textures/Checker.png",
                b"owned alternative original".to_vec(),
            ),
            ("Textures/Stone_Base_Color.tga", tga.clone()),
            ("Textures/Stone_Roughness.tga", tga.clone()),
            ("Sprites/Run_0001.tga", tga.clone()),
            ("Sprites/Run_0002.tga", tga.clone()),
            (
                "Model.obj",
                b"mtllib Model.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Owned\nf 1 2 3\n".to_vec(),
            ),
            (
                "Model.mtl",
                b"newmtl Owned\nmap_Kd Textures/Checker.tga\n".to_vec(),
            ),
        ]);
        let archive = workspace.path("TGA workflows.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for (name, bytes) in &originals {
            fs::create_dir_all(source.join(Path::new(name).parent().unwrap())).unwrap();
            fs::write(source.join(name), bytes).unwrap();
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let original_zip = fs::read(&archive).unwrap();
        let mut library = Library::create(&root).unwrap();
        let packages = [
            library.import_folder(&source).unwrap(),
            library.reference_folder(&source).unwrap(),
            library.import_zip(&archive).unwrap(),
        ];
        drop(library);
        let mut library = Library::open(&root).unwrap();
        for package in packages {
            let images = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    kind: Some(AssetKind::Image),
                    format: Some("TGA".into()),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(images.total, 5);
            let checker = images
                .items
                .iter()
                .find(|asset| asset.name == "Checker")
                .unwrap();
            assert_eq!(checker.files.len(), 2);
            let checker_tga = checker
                .files
                .iter()
                .find(|file| file.format == "TGA")
                .unwrap();
            let direct = workspace.path(&format!("Checker-{}.tga", package.id));
            library
                .export(checker_tga.id, Some(&direct), false)
                .unwrap();
            assert_eq!(fs::read(direct).unwrap(), tga);
            for workflow in [ImageWorkflow::Material, ImageWorkflow::SpriteSequence] {
                let page = library
                    .query(CatalogQuery {
                        package_id: Some(package.id),
                        workflow: Some(workflow),
                        ..Default::default()
                    })
                    .unwrap();
                assert_eq!(page.total, 1);
                let group = library
                    .image_group(page.items[0].files[0].id, workflow)
                    .unwrap();
                assert_eq!(group.members.len(), 2);
                assert!(group.resources.errors.is_empty());
                let paths = group
                    .members
                    .iter()
                    .map(|member| member.resource_path.clone())
                    .collect::<Vec<_>>();
                let destination = workspace.path(&format!("Group-{}-{:?}", package.id, workflow));
                library
                    .export_model(&group.resources, &paths, Some(&destination))
                    .unwrap();
                for path in paths {
                    assert_eq!(fs::read(destination.join(path)).unwrap(), tga);
                }
            }
            let models = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    kind: Some(AssetKind::Model),
                    ..Default::default()
                })
                .unwrap();
            let snapshot = library
                .model_resources(models.items[0].files[0].id)
                .unwrap();
            let paths = vec!["Model.mtl".into(), "Textures/Checker.tga".into()];
            let destination = workspace.path(&format!("Model-{}", package.id));
            library
                .export_model(&snapshot, &paths, Some(&destination))
                .unwrap();
            for name in ["Model.obj", "Model.mtl", "Textures/Checker.tga"] {
                assert_eq!(fs::read(destination.join(name)).unwrap(), originals[name]);
            }
            assert!(!destination.join("Textures/Checker.png").exists());
            library.trash_file(checker_tga.id).unwrap();
            assert!(library.resolve_preview(checker_tga.id).is_err());
            let rejected = workspace.path(&format!("Trashed-model-{}", package.id));
            assert!(library
                .export_model(&snapshot, &paths, Some(&rejected))
                .is_err());
            assert!(!rejected.exists());
        }
        for (name, bytes) in originals {
            assert_eq!(fs::read(source.join(name)).unwrap(), bytes);
        }
        assert_eq!(fs::read(archive).unwrap(), original_zip);
    }

    #[test]
    fn mixed_catalog_filters_kinds_and_preserves_existing_sound_records_after_reopen() {
        let workspace = Workspace::new();
        let source = workspace.path("Source");
        let root = workspace.path("library");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("Beacon.mp3"),
            include_bytes!("../tests/fixtures/audio/stereo44100.mp3"),
        )
        .unwrap();
        let mut library = Library::create(&root).unwrap();
        let old_package = library.import_folder(&source).unwrap();
        let old_sound = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        let folder = library.create_folder("Salzburg Protocol", None).unwrap();
        library
            .annotate_asset(old_sound.id, "Beacon effect", &["ui".into()])
            .unwrap();
        library.add_to_folder(folder.id, old_sound.id).unwrap();
        library.analyze_audio(old_sound.files[0].id).unwrap();
        for filename in [
            "Beacon.png",
            "Beacon.jpg",
            "Beacon.psd",
            "Beacon.glb",
            "Beacon.obj",
        ] {
            fs::write(source.join(filename), filename.as_bytes()).unwrap();
        }
        assert_eq!(
            library.query(CatalogQuery::default()).unwrap().total,
            1,
            "existing packages are not silently rescanned"
        );
        let mixed = library.import_folder(&source).unwrap();
        drop(library);
        drop(Library::open(&root).unwrap());
        let before = fs::read(root.join("catalog.sqlite3")).unwrap();
        let library = Library::open_read_only(&root).unwrap();
        for (kind, expected, variants) in [
            (AssetKind::Sound, 2, 1),
            (AssetKind::Image, 1, 3),
            (AssetKind::Model, 1, 2),
        ] {
            let page = library
                .query(CatalogQuery {
                    kind: Some(kind),
                    limit: 1,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(page.total, expected);
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].kind, kind);
            assert_eq!(page.items[0].files.len(), variants);
            assert!(page.items[0].files.iter().all(|file| file.kind == kind));
        }
        let image = library
            .query(CatalogQuery {
                kind: Some(AssetKind::Image),
                format: Some("jpeg".into()),
                package_id: Some(mixed.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(image.total, 1);
        assert_eq!(
            image.items[0].files.len(),
            3,
            "format filters retain the asset's variants"
        );
        let psd_page = library
            .query(CatalogQuery {
                kind: Some(AssetKind::Image),
                format: Some("psd".into()),
                package_id: Some(mixed.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(psd_page.total, 1);
        assert_eq!(psd_page.items[0].id, image.items[0].id);
        let psd = image.items[0]
            .files
            .iter()
            .find(|file| file.format == "PSD")
            .unwrap();
        assert_eq!(psd.kind, AssetKind::Image);
        assert_eq!(
            fs::read(library.resolve_preview(psd.id).unwrap()).unwrap(),
            b"Beacon.psd"
        );
        assert_eq!(
            library
                .query(CatalogQuery {
                    kind: Some(AssetKind::Model),
                    format: Some("JPEG".into()),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
        let old = library
            .query(CatalogQuery {
                asset_id: Some(old_sound.id),
                folder_id: Some(folder.id),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        assert_eq!(old.package_id, old_package.id);
        assert_eq!(old.name, "Beacon effect");
        assert_eq!(old.tags, ["ui"]);
        assert_eq!(old.folder_ids, [folder.id]);
        assert_eq!(old.files[0].id, old_sound.files[0].id);
        assert_eq!(
            old.files[0].audio.as_ref().unwrap().status,
            AudioStatus::Ready
        );
        assert_eq!(old.kind, AssetKind::Sound);
        drop(library);
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    }

    #[test]
    fn variant_corrections_reject_different_kinds_without_changing_annotations_or_memberships() {
        let workspace = Workspace::new();
        let source = workspace.path("Mixed");
        fs::create_dir(&source).unwrap();
        for filename in ["Beacon.mp3", "Beacon.png", "Beacon.glb"] {
            fs::write(source.join(filename), filename.as_bytes()).unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let originals = library.query(CatalogQuery::default()).unwrap().items;
        for asset in &originals {
            library
                .annotate_asset(asset.id, "Named asset", &[asset.files[0].format.clone()])
                .unwrap();
            let folder = library.create_folder(&asset.files[0].format, None).unwrap();
            library.add_to_folder(folder.id, asset.id).unwrap();
        }
        let before = library.query(CatalogQuery::default()).unwrap().items;
        for source in &before {
            for target in before.iter().filter(|asset| asset.id != source.id) {
                let error = library
                    .assign_variant(source.files[0].id, target.id)
                    .unwrap_err();
                assert!(error.contains("Asset-Typen"), "{error}");
            }
        }
        let after = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(after.total, 3);
        for original in before {
            let current = after
                .items
                .iter()
                .find(|asset| asset.id == original.id)
                .unwrap();
            assert_eq!(current.name, original.name);
            assert_eq!(current.kind, original.kind);
            assert_eq!(current.tags, original.tags);
            assert_eq!(current.folder_ids, original.folder_ids);
            assert_eq!(current.files.len(), 1);
            assert_eq!(current.files[0].id, original.files[0].id);
            assert_eq!(
                fs::read(library.resolve_preview(current.files[0].id).unwrap()).unwrap(),
                current.files[0].original_filename.as_bytes()
            );
        }
    }

    #[test]
    fn visual_files_reject_audio_analysis_without_caching_errors_and_keep_image_export() {
        let workspace = Workspace::new();
        let source = workspace.path("Mixed");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("Beacon.mp3"),
            include_bytes!("../tests/fixtures/audio/stereo44100.mp3"),
        )
        .unwrap();
        fs::write(source.join("Beacon.png"), b"original image").unwrap();
        fs::write(source.join("Beacon.glb"), b"original model").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        for asset in library.query(CatalogQuery::default()).unwrap().items {
            if asset.kind == AssetKind::Sound {
                assert_eq!(
                    library.analyze_audio(asset.files[0].id).unwrap().status,
                    AudioStatus::Ready
                );
            } else {
                assert!(library
                    .analyze_audio(asset.files[0].id)
                    .unwrap_err()
                    .contains("kein Sound"));
            }
            if asset.kind == AssetKind::Image {
                let destination = workspace.path("Export.png");
                assert!(matches!(
                    library
                        .export(asset.files[0].id, Some(&destination), false)
                        .unwrap(),
                    ExportOutcome::Exported { .. }
                ));
                assert_eq!(fs::read(destination).unwrap(), b"original image");
            }
        }
        let page = library.query(CatalogQuery::default()).unwrap();
        assert!(page
            .items
            .iter()
            .filter(|asset| asset.kind != AssetKind::Sound)
            .all(|asset| asset.files[0].audio.is_none()));
        assert_eq!(page.total, 3);
    }

    #[test]
    fn read_only_catalog_reads_annotations_and_rejects_writes_without_changing_bytes() {
        let workspace = Workspace::new();
        let root = workspace.path("library");
        let source = workspace.path("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("River.wav"), b"original sound").unwrap();
        let mut library = Library::create(&root).unwrap();
        library.import_folder(&source).unwrap();
        let asset = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        library
            .annotate_asset(asset.id, "River ambience", &["water".into()])
            .unwrap();
        let path = library.resolve_preview(asset.files[0].id).unwrap();
        drop(library);
        let before = fs::read(root.join("catalog.sqlite3")).unwrap();
        let mut read_only = Library::open_read_only(&root).unwrap();
        let saved = read_only.query(CatalogQuery::default()).unwrap();
        assert_eq!(saved.items[0].name, "River ambience");
        assert_eq!(saved.items[0].tags, ["water"]);
        assert_eq!(read_only.resolve_preview(asset.files[0].id).unwrap(), path);
        assert!(read_only.annotate_asset(asset.id, "Changed", &[]).is_err());
        assert!(read_only.create_folder("Changed", None).is_err());
        drop(read_only);
        assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
        assert_eq!(fs::read(path).unwrap(), b"original sound");
        assert_eq!(
            fs::read_dir(root).unwrap().count(),
            2,
            "no backup or migration file is created"
        );
    }

    #[test]
    fn project_folders_collect_references_and_filter_descendants_after_reopen() {
        let workspace = Workspace::new();
        let source = workspace.path("Original package");
        fs::create_dir_all(source.join("water")).unwrap();
        fs::write(source.join("water/river.wav"), b"original wave").unwrap();
        fs::write(source.join("water/river.ogg"), b"original ogg").unwrap();
        fs::write(source.join("impact.wav"), b"original impact").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.import_folder(&source).unwrap();
        let before = library.query(CatalogQuery::default()).unwrap();
        let river = before
            .items
            .iter()
            .find(|item| item.name == "river")
            .unwrap();
        let original_path = library.resolve_preview(river.files[0].id).unwrap();
        library
            .annotate_asset(river.id, "Ruhiger Bach", &["water".into()])
            .unwrap();

        let project = library.create_folder("Salzburg Protocol", None).unwrap();
        let sounds = library.create_folder("Sounds", Some(project.id)).unwrap();
        let ambient = library.create_folder("Ambient", Some(sounds.id)).unwrap();
        let other = library.create_folder("Another Project", None).unwrap();
        for folder_id in [sounds.id, ambient.id, other.id, sounds.id] {
            library.add_to_folder(folder_id, river.id).unwrap();
        }
        let folders = library.folders().unwrap();
        assert_eq!(folders.len(), 4);
        assert_eq!(
            folders
                .iter()
                .find(|item| item.id == sounds.id)
                .unwrap()
                .parent_id,
            Some(project.id)
        );
        assert!(
            folders.iter().all(|item| item.asset_count == 1),
            "counts include distinct assets in descendants"
        );
        for folder_id in [project.id, sounds.id, ambient.id, other.id] {
            let page = library
                .query(CatalogQuery {
                    folder_id: Some(folder_id),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(
                page.total, 1,
                "multiple descendant memberships must not duplicate search results"
            );
            assert_eq!(page.items[0].id, river.id);
        }
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 2);
        drop(library);

        let mut library = Library::open(&root).unwrap();
        let page = library
            .query(CatalogQuery {
                folder_id: Some(project.id),
                ..Default::default()
            })
            .unwrap();
        let saved = &page.items[0];
        assert_eq!(saved.name, "Ruhiger Bach");
        assert_eq!(saved.tags, ["water"]);
        assert_eq!(saved.folder_ids, [sounds.id, ambient.id, other.id]);
        assert_eq!(saved.files.len(), 2);
        assert_eq!(saved.files[0].relative_path, river.files[0].relative_path);
        assert_eq!(
            saved.files[0].original_filename,
            river.files[0].original_filename
        );
        assert_eq!(saved.files[0].original_folder, "water");
        assert_eq!(
            library.resolve_preview(saved.files[0].id).unwrap(),
            original_path
        );
        assert_eq!(fs::read(original_path).unwrap(), b"original ogg");
        assert_eq!(
            fs::read(source.join("water/river.ogg")).unwrap(),
            b"original ogg"
        );
        assert_eq!(
            library.packages().unwrap()[0].relative_path,
            package.relative_path
        );

        library.remove_from_folder(sounds.id, river.id).unwrap();
        assert_eq!(
            library
                .query(CatalogQuery {
                    folder_id: Some(project.id),
                    ..Default::default()
                })
                .unwrap()
                .total,
            1
        );
        library.remove_from_folder(ambient.id, river.id).unwrap();
        assert_eq!(
            library
                .query(CatalogQuery {
                    folder_id: Some(project.id),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            library
                .query(CatalogQuery {
                    folder_id: Some(other.id),
                    ..Default::default()
                })
                .unwrap()
                .total,
            1
        );
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 2);
        drop(library);
        let library = Library::open(&root).unwrap();
        let folders = library.folders().unwrap();
        assert_eq!(
            folders
                .iter()
                .find(|item| item.id == project.id)
                .unwrap()
                .asset_count,
            0
        );
        assert_eq!(
            folders
                .iter()
                .find(|item| item.id == other.id)
                .unwrap()
                .asset_count,
            1
        );
    }

    #[test]
    fn project_folders_reject_invalid_names_and_unknown_or_trashed_assets() {
        let workspace = Workspace::new();
        let source = workspace.path("Source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("river.wav"), b"retained original").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let asset = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        for name in [
            "",
            "   ",
            ".",
            " .. ",
            "a/b",
            "a\\b",
            "line\nbreak",
            "\tSounds",
            "sound\0",
            &"ü".repeat(257),
        ] {
            assert!(
                library.create_folder(name, None).is_err(),
                "invalid name accepted: {name:?}"
            );
        }
        assert!(library.folders().unwrap().is_empty());
        assert!(library.create_folder("Missing parent", Some(-1)).is_err());
        assert!(
            library.create_folder("Self parent", Some(1)).is_err(),
            "the parent must exist before insertion, even if its ID would match the new folder"
        );
        assert!(library.folders().unwrap().is_empty());
        let folder = library
            .create_folder("  Salzburg Protocol  ", None)
            .unwrap();
        assert_eq!(folder.name, "Salzburg Protocol");
        assert_eq!(library.folders().unwrap().len(), 1);
        assert!(library.add_to_folder(-1, asset.id).is_err());
        assert!(library.add_to_folder(folder.id, -1).is_err());
        assert!(library.remove_from_folder(-1, asset.id).is_err());
        assert!(library.remove_from_folder(folder.id, -1).is_err());
        library.add_to_folder(folder.id, asset.id).unwrap();
        library.trash_file(asset.files[0].id).unwrap();
        assert!(library.add_to_folder(folder.id, asset.id).is_err());
        assert!(library.remove_from_folder(folder.id, asset.id).is_err());
        assert_eq!(library.folders().unwrap()[0].asset_count, 0);
        let trash_id = library.trash_entries().unwrap()[0].id;
        library.restore_trash(trash_id).unwrap();
        let restored = library
            .query(CatalogQuery {
                folder_id: Some(folder.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(restored.items[0].folder_ids, [folder.id]);
        assert_eq!(
            fs::read(library.resolve_preview(asset.files[0].id).unwrap()).unwrap(),
            b"retained original"
        );
        assert_eq!(
            library
                .query(CatalogQuery {
                    folder_id: Some(-1),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
    }

    #[test]
    fn project_folders_follow_variant_corrections_and_recover_hidden_memberships() {
        let workspace = Workspace::new();
        let source = workspace.path("Variant package");
        fs::create_dir_all(source.join("water")).unwrap();
        fs::create_dir_all(source.join("effects")).unwrap();
        fs::write(source.join("water/River.wav"), b"wave representation").unwrap();
        fs::write(source.join("water/River.ogg"), b"ogg representation").unwrap();
        fs::write(source.join("water/River.flac"), b"flac representation").unwrap();
        fs::write(source.join("effects/Impact.wav"), b"impact representation").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.import_folder(&source).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        let river = page.items.iter().find(|item| item.name == "River").unwrap();
        let target = page
            .items
            .iter()
            .find(|item| item.name == "Impact")
            .unwrap();
        let wav = river
            .files
            .iter()
            .find(|file| file.format == "WAV")
            .unwrap();
        let ogg = river
            .files
            .iter()
            .find(|file| file.format == "OGG")
            .unwrap();
        let flac = river
            .files
            .iter()
            .find(|file| file.format == "FLAC")
            .unwrap();
        library
            .annotate_asset(river.id, "Quiet river", &["water".into()])
            .unwrap();
        let project = library.create_folder("Salzburg Protocol", None).unwrap();
        let sounds = library.create_folder("Sounds", Some(project.id)).unwrap();
        let effects = library.create_folder("Effects", Some(project.id)).unwrap();
        let other = library.create_folder("Another Project", None).unwrap();
        library.add_to_folder(sounds.id, river.id).unwrap();
        library.add_to_folder(other.id, river.id).unwrap();
        library.add_to_folder(effects.id, target.id).unwrap();
        library.trash_file(ogg.id).unwrap();
        let split_id = library.split_variant(wav.id).unwrap();
        let split = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .into_iter()
            .find(|item| item.id == split_id)
            .unwrap();
        assert_eq!(
            split.folder_ids,
            [sounds.id, other.id],
            "a split inherits the original asset memberships"
        );
        library.assign_variant(wav.id, target.id).unwrap();
        let merged = library
            .query(CatalogQuery {
                folder_id: Some(effects.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            merged.items[0].folder_ids,
            [sounds.id, effects.id, other.id],
            "reassignment unions source and target folders"
        );
        assert_eq!(
            library
                .query(CatalogQuery {
                    folder_id: Some(sounds.id),
                    ..Default::default()
                })
                .unwrap()
                .total,
            2
        );
        library.assign_variant(flac.id, target.id).unwrap();
        assert_eq!(
            library
                .query(CatalogQuery {
                    folder_id: Some(sounds.id),
                    ..Default::default()
                })
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            library
                .folders()
                .unwrap()
                .iter()
                .find(|item| item.id == project.id)
                .unwrap()
                .asset_count,
            1
        );
        drop(library);

        let mut library = Library::open(&root).unwrap();
        let trash_id = library
            .trash_entries()
            .unwrap()
            .iter()
            .find(|entry| entry.file_id == Some(ogg.id))
            .unwrap()
            .id;
        library.restore_trash(trash_id).unwrap();
        let restored = library
            .query(CatalogQuery {
                folder_id: Some(sounds.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            restored.total, 2,
            "restoring a hidden sibling retains its original asset membership"
        );
        let original = restored
            .items
            .iter()
            .find(|item| item.id == river.id)
            .unwrap();
        assert_eq!(original.name, "Quiet river");
        assert_eq!(original.tags, ["water"]);
        assert_eq!(original.folder_ids, [sounds.id, other.id]);
        assert_eq!(original.files[0].id, ogg.id);
        library.trash_package(package.id).unwrap();
        assert!(library
            .folders()
            .unwrap()
            .iter()
            .all(|item| item.asset_count == 0));
        assert_eq!(
            library
                .query(CatalogQuery {
                    folder_id: Some(project.id),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
        drop(library);

        let mut library = Library::open(&root).unwrap();
        let trash_id = library
            .trash_entries()
            .unwrap()
            .iter()
            .find(|entry| entry.file_id.is_none())
            .unwrap()
            .id;
        library.restore_trash(trash_id).unwrap();
        let final_page = library
            .query(CatalogQuery {
                folder_id: Some(project.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(final_page.total, 2);
        assert_eq!(
            final_page
                .items
                .iter()
                .find(|item| item.id == target.id)
                .unwrap()
                .folder_ids,
            [sounds.id, effects.id, other.id]
        );
        for original in page.items.iter().flat_map(|asset| &asset.files) {
            let saved = final_page
                .items
                .iter()
                .flat_map(|asset| &asset.files)
                .find(|file| file.id == original.id)
                .unwrap();
            assert_eq!(saved.relative_path, original.relative_path);
            assert_eq!(saved.original_filename, original.original_filename);
            assert_eq!(saved.original_folder, original.original_folder);
            assert_eq!(
                fs::read(library.resolve_preview(saved.id).unwrap()).unwrap(),
                fs::read(
                    source
                        .join(&saved.original_folder)
                        .join(&saved.original_filename)
                )
                .unwrap()
            );
        }
    }

    #[test]
    fn trash_file_hides_only_that_variant_and_restores_annotations_after_reopen() {
        let workspace = Workspace::new();
        let source = workspace.path("Water Pack");
        fs::create_dir_all(source.join("solo")).unwrap();
        fs::write(source.join("river.wav"), b"original wav").unwrap();
        fs::write(source.join("river.ogg"), b"original ogg").unwrap();
        fs::write(source.join("solo/whistle.flac"), b"original flac").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.import_folder(&source).unwrap();
        let before = library.query(CatalogQuery::default()).unwrap();
        let river = before
            .items
            .iter()
            .find(|item| item.name == "river")
            .unwrap();
        let wav = river
            .files
            .iter()
            .find(|file| file.format == "WAV")
            .unwrap();
        let ogg = river
            .files
            .iter()
            .find(|file| file.format == "OGG")
            .unwrap();
        let whistle = before
            .items
            .iter()
            .find(|item| item.name == "whistle")
            .unwrap();
        let whistle_file = &whistle.files[0];
        let wav_path = library.resolve_preview(wav.id).unwrap();
        let ogg_path = library.resolve_preview(ogg.id).unwrap();
        library
            .annotate_asset(river.id, "Quiet river", &["water".into()])
            .unwrap();
        library
            .annotate_asset(whistle.id, "Whistle", &["solo tag".into()])
            .unwrap();
        library
            .annotate_package(package.id, &["nature".into()])
            .unwrap();
        let audio = library.analyze_audio(wav.id).unwrap();

        library.trash_file(wav.id).unwrap();
        assert!(library.trash_file(-1).is_err());
        assert!(library.trash_file(wav.id).is_err());
        assert!(library.resolve_preview(wav.id).is_err());
        assert!(library.analyze_audio(wav.id).is_err());
        let export = workspace.path("deleted.wav");
        assert!(library.export(wav.id, Some(&export), false).is_err());
        assert!(!export.exists());
        assert!(library.split_variant(wav.id).is_err());
        assert!(library.assign_variant(wav.id, whistle.id).is_err());
        assert!(
            library.split_variant(ogg.id).is_err(),
            "hidden variants cannot count toward splitting"
        );
        let visible = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(visible.total, 2);
        let visible_river = visible
            .items
            .iter()
            .find(|item| item.id == river.id)
            .unwrap();
        assert_eq!(visible_river.files.len(), 1);
        assert_eq!(visible_river.files[0].id, ogg.id);
        assert_eq!(library.packages().unwrap()[0].asset_count, 2);
        assert_eq!(
            library.catalog_filters(None).unwrap().formats,
            ["FLAC", "OGG"]
        );
        for query in [
            CatalogQuery {
                text: "river.wav".into(),
                ..Default::default()
            },
            CatalogQuery {
                format: Some("WAV".into()),
                ..Default::default()
            },
        ] {
            assert_eq!(library.query(query).unwrap().total, 0);
        }
        assert_eq!(library.resolve_preview(ogg.id).unwrap(), ogg_path);
        assert_eq!(fs::read(&wav_path).unwrap(), b"original wav");
        assert_eq!(fs::read(source.join("river.wav")).unwrap(), b"original wav");

        library.trash_file(whistle_file.id).unwrap();
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 1);
        assert_eq!(library.packages().unwrap()[0].asset_count, 1);
        let filters = library.catalog_filters(Some(package.id)).unwrap();
        assert_eq!(filters.folders, [""]);
        assert_eq!(filters.formats, ["OGG"]);
        assert_eq!(filters.tags, ["nature", "water"]);
        assert!(library.assign_variant(ogg.id, whistle.id).is_err());
        assert!(library.annotate_asset(whistle.id, "Changed", &[]).is_err());
        drop(library);

        let mut library = Library::open(&root).unwrap();
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 1);
        let trash = library.trash_entries().unwrap();
        assert_eq!(trash.len(), 2);
        let entry = trash
            .iter()
            .find(|entry| entry.file_id == Some(wav.id))
            .unwrap();
        assert_eq!(entry.package_id, package.id);
        assert_eq!(entry.package_name, "Water Pack");
        assert_eq!(entry.name, "river.wav");
        assert!(entry.deleted_at > 0);
        assert!(!entry.parent_trashed);
        assert!(library.restore_trash(-1).is_err());
        for entry in trash {
            library.restore_trash(entry.id).unwrap();
        }
        assert!(library.trash_entries().unwrap().is_empty());
        let restored = library.query(CatalogQuery::default()).unwrap();
        let river = restored
            .items
            .iter()
            .find(|item| item.id == river.id)
            .unwrap();
        assert_eq!(river.name, "Quiet river");
        assert_eq!(river.tags, ["water"]);
        assert_eq!(river.files.len(), 2);
        let restored_wav = river.files.iter().find(|file| file.id == wav.id).unwrap();
        assert_eq!(restored_wav.relative_path, wav.relative_path);
        assert_eq!(restored_wav.original_filename, "river.wav");
        assert_eq!(restored_wav.audio.as_ref().unwrap().error, audio.error);
        let whistle = restored
            .items
            .iter()
            .find(|item| item.id == whistle.id)
            .unwrap();
        assert_eq!(whistle.name, "Whistle");
        assert_eq!(whistle.tags, ["solo tag"]);
        assert_eq!(library.packages().unwrap()[0].tags, ["nature"]);
        assert_eq!(library.resolve_preview(wav.id).unwrap(), wav_path);
        library.export(wav.id, Some(&export), false).unwrap();
        assert_eq!(fs::read(export).unwrap(), b"original wav");

        library.trash_file(wav.id).unwrap();
        let trash_id = library.trash_entries().unwrap()[0].id;
        library.assign_variant(ogg.id, whistle.id).unwrap();
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 1);
        drop(library);
        let mut library = Library::open(&root).unwrap();
        library.restore_trash(trash_id).unwrap();
        let regrouped = library.query(CatalogQuery::default()).unwrap();
        let river = regrouped
            .items
            .iter()
            .find(|item| item.id == river.id)
            .unwrap();
        assert_eq!(river.name, "Quiet river");
        assert_eq!(river.tags, ["water"]);
        assert_eq!(river.files.len(), 1);
        assert_eq!(river.files[0].id, wav.id);
        let whistle = regrouped
            .items
            .iter()
            .find(|item| item.id == whistle.id)
            .unwrap();
        assert_eq!(whistle.name, "Whistle");
        assert_eq!(whistle.files.len(), 2);
        assert_eq!(library.resolve_preview(ogg.id).unwrap(), ogg_path);
    }

    #[test]
    fn empty_trash_deletes_managed_data_and_only_catalog_rows_of_external_packages() {
        let workspace = Workspace::new();
        let managed_source = workspace.path("Managed");
        let kept_source = workspace.path("Kept");
        let external = workspace.path("External");
        for (folder, file) in [
            (&managed_source, "rain.wav"),
            (&kept_source, "wind.wav"),
            (&kept_source, "fire.wav"),
            (&external, "river.wav"),
        ] {
            fs::create_dir_all(folder).unwrap();
            fs::write(folder.join(file), file).unwrap();
        }
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let managed = library.import_folder(&managed_source).unwrap();
        let kept = library.import_folder(&kept_source).unwrap();
        let referenced = library.reference_folder(&external).unwrap();
        let items = library.query(CatalogQuery::default()).unwrap().items;
        let rain = items.iter().find(|item| item.name == "rain").unwrap();
        library
            .annotate_asset(rain.id, "rain", &["weather".into()])
            .unwrap();
        let wind = items.iter().find(|item| item.name == "wind").unwrap().files[0].id;
        let wind_path = library.resolve_preview(wind).unwrap();
        library.trash_package(managed.id).unwrap();
        library.trash_package(referenced.id).unwrap();
        library.trash_file(wind).unwrap();

        let emptied = library.empty_trash().unwrap();

        assert_eq!((emptied.packages, emptied.files), (2, 1));
        assert!(emptied.disk_errors.is_empty(), "{:?}", emptied.disk_errors);
        assert!(!root.join(&managed.relative_path).exists());
        assert!(!wind_path.exists());
        assert!(root.join(&kept.relative_path).join("fire.wav").exists());
        assert_eq!(fs::read(external.join("river.wav")).unwrap(), b"river.wav");
        let count = |table: &str| -> i64 {
            library
                .connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        for (table, expected) in [
            ("packages", 1),
            ("assets", 1),
            ("asset_files", 1),
            ("trash", 0),
            ("asset_tags", 0),
            ("external_sources", 0),
            ("external_files", 0),
        ] {
            assert_eq!(count(table), expected, "{table}");
        }
    }

    #[test]
    fn trash_packages_keep_zip_and_external_bytes_and_can_be_reimported_before_restore() {
        use std::io::Write;
        let workspace = Workspace::new();
        let source = workspace.path("External Water");
        fs::create_dir_all(source.join("Water")).unwrap();
        fs::write(source.join("Water/river.wav"), b"river wav").unwrap();
        fs::write(source.join("Water/river.ogg"), b"river ogg").unwrap();
        fs::write(source.join("solo.flac"), b"solo flac").unwrap();
        let archive = workspace.path("Managed Water.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for (name, bytes) in [
            ("Water/river.wav", b"river wav".as_slice()),
            ("Water/river.ogg", b"river ogg".as_slice()),
            ("solo.flac", b"solo flac".as_slice()),
            ("LICENSE.txt", b"original license".as_slice()),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let archive_bytes = fs::read(&archive).unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let packages = [
            library.import_zip(&archive).unwrap(),
            library.reference_folder(&source).unwrap(),
        ];
        assert!(library.trash_package(-1).is_err());

        for package in packages {
            let before = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    ..Default::default()
                })
                .unwrap();
            let river = before
                .items
                .iter()
                .find(|item| item.name == "river")
                .unwrap();
            let wav = river
                .files
                .iter()
                .find(|file| file.format == "WAV")
                .unwrap();
            let ogg = river
                .files
                .iter()
                .find(|file| file.format == "OGG")
                .unwrap();
            let path = library.resolve_preview(wav.id).unwrap();
            let stored = library.analyze_audio(wav.id).unwrap();
            library
                .annotate_asset(river.id, "Named river", &["water".into()])
                .unwrap();
            library
                .annotate_package(package.id, &["package tag".into()])
                .unwrap();
            library.trash_file(ogg.id).unwrap();
            let deleted_at = library.trash_entries().unwrap()[0].deleted_at;

            library.trash_package(package.id).unwrap();
            assert!(library.trash_package(package.id).is_err());
            assert!(library.trash_file(wav.id).is_err());
            assert!(!library
                .packages()
                .unwrap()
                .iter()
                .any(|item| item.id == package.id));
            assert_eq!(
                library
                    .query(CatalogQuery {
                        package_id: Some(package.id),
                        ..Default::default()
                    })
                    .unwrap()
                    .total,
                0
            );
            let filters = library.catalog_filters(Some(package.id)).unwrap();
            assert!(
                filters.folders.is_empty() && filters.formats.is_empty() && filters.tags.is_empty()
            );
            assert!(library.resolve_preview(wav.id).is_err());
            assert!(library.analyze_audio(wav.id).is_err());
            assert!(library.annotate_asset(river.id, "Changed", &[]).is_err());
            assert!(library.annotate_package(package.id, &[]).is_err());
            assert!(library.split_variant(wav.id).is_err());
            let exported = workspace.path("hidden.wav");
            assert!(library.export(wav.id, Some(&exported), false).is_err());
            assert!(library.export(wav.id, None, false).is_err());
            assert!(!exported.exists());
            assert_eq!(fs::read(&path).unwrap(), b"river wav");
            assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
            assert_eq!(
                fs::read(source.join("Water/river.ogg")).unwrap(),
                b"river ogg"
            );
            if package.source_kind == "managed" {
                assert_eq!(
                    fs::read(root.join(&package.relative_path).join("LICENSE.txt")).unwrap(),
                    b"original license"
                );
            } else {
                assert!(library.relink_source(package.id, &source).is_err());
            }
            let reimported = if package.source_kind == "managed" {
                library.import_zip(&archive).unwrap()
            } else {
                library.reference_folder(&source).unwrap()
            };
            assert_ne!(reimported.id, package.id);
            assert_ne!(reimported.relative_path, package.relative_path);
            assert_eq!(reimported.asset_count, 2);
            drop(library);
            library = Library::open(&root).unwrap();

            let entries = library.trash_entries().unwrap();
            let file_entry = entries
                .iter()
                .find(|entry| entry.file_id == Some(ogg.id))
                .unwrap();
            let package_entry = entries
                .iter()
                .find(|entry| entry.file_id.is_none() && entry.package_id == package.id)
                .unwrap();
            assert!(file_entry.parent_trashed);
            assert!(!package_entry.parent_trashed);
            assert_eq!(file_entry.deleted_at, deleted_at);
            assert_eq!(file_entry.original_folder.as_deref(), Some("Water"));
            assert_eq!(package_entry.name, package.name);
            assert!(
                library.restore_trash(file_entry.id).is_err(),
                "restoring a hidden child must require its package first"
            );
            library.restore_trash(package_entry.id).unwrap();
            assert_eq!(
                library
                    .query(CatalogQuery {
                        package_id: Some(package.id),
                        ..Default::default()
                    })
                    .unwrap()
                    .total,
                2
            );
            assert!(
                library.resolve_preview(ogg.id).is_err(),
                "package restore preserves individually trashed files"
            );
            assert!(!library.trash_entries().unwrap()[0].parent_trashed);
            library.restore_trash(file_entry.id).unwrap();
            let restored = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    ..Default::default()
                })
                .unwrap();
            let river = restored
                .items
                .iter()
                .find(|item| item.id == river.id)
                .unwrap();
            assert_eq!(river.name, "Named river");
            assert_eq!(river.tags, ["water"]);
            assert_eq!(river.files.len(), 2);
            assert_eq!(
                river
                    .files
                    .iter()
                    .find(|file| file.id == wav.id)
                    .unwrap()
                    .audio
                    .as_ref()
                    .unwrap()
                    .error,
                stored.error
            );
            assert_eq!(
                library
                    .packages()
                    .unwrap()
                    .iter()
                    .find(|item| item.id == package.id)
                    .unwrap()
                    .tags,
                ["package tag"]
            );
            assert_eq!(library.resolve_preview(wav.id).unwrap(), path);
            assert!(library.trash_entries().unwrap().is_empty());
        }
        assert_eq!(library.packages().unwrap().len(), 4);
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 8);
        assert_eq!(fs::read(archive).unwrap(), archive_bytes);
    }

    #[cfg(target_os = "macos")]
    struct TestImages {
        mounts: Vec<PathBuf>,
        _device_slot: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(target_os = "macos")]
    impl Default for TestImages {
        fn default() -> Self {
            static DEVICE_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());
            Self {
                mounts: Vec::new(),
                _device_slot: DEVICE_SLOT
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()),
            }
        }
    }

    #[cfg(target_os = "macos")]
    impl TestImages {
        fn create(path: &Path) {
            let output = std::process::Command::new("hdiutil")
                .args([
                    "create",
                    "-quiet",
                    "-size",
                    "256m",
                    "-fs",
                    "APFS",
                    "-type",
                    "SPARSE",
                    "-volname",
                    "Game Asset Test Source",
                ])
                .arg(path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "owned test image creation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fn attach(&mut self, image: &Path, mount: &Path) {
            fs::create_dir_all(mount).unwrap();
            let output = std::process::Command::new("hdiutil")
                .args([
                    "attach",
                    "-quiet",
                    "-nobrowse",
                    "-noautoopen",
                    "-mountpoint",
                ])
                .arg(mount)
                .arg(image)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "owned test image attachment failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            self.mounts.push(mount.to_owned());
        }
        fn detach(&mut self, mount: &Path) {
            let output = std::process::Command::new("hdiutil")
                .args(["detach", "-quiet"])
                .arg(mount)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "owned test image detachment failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            self.mounts.retain(|path| path != mount);
        }
    }
    #[cfg(target_os = "macos")]
    impl Drop for TestImages {
        fn drop(&mut self) {
            for mount in &self.mounts {
                let _ = std::process::Command::new("hdiutil")
                    .args(["detach", "-quiet"])
                    .arg(mount)
                    .output();
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_unchanged_attached_source_survives_a_real_device_number_change_and_relink() {
        use std::os::unix::fs::MetadataExt;
        let workspace = Workspace::new();
        let mut images = TestImages::default();
        let image = workspace.path("source.sparseimage");
        let filler = workspace.path("filler.sparseimage");
        let mount = workspace.path("mounted-source");
        let filler_mount = workspace.path("mounted-filler");
        TestImages::create(&image);
        TestImages::create(&filler);
        images.attach(&image, &mount);
        let source = mount.join("LibrarySource");
        fs::create_dir(&source).unwrap();
        let original = b"original attached-drive bytes";
        fs::write(source.join("A001.wav"), original).unwrap();
        let before = fs::metadata(&source).unwrap();
        let before_file = fs::metadata(source.join("A001.wav")).unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.reference_folder(&source).unwrap();
        let asset = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        let file_id = asset.files[0].id;
        library
            .annotate_asset(asset.id, "RemountProbe", &["laufwerk".into()])
            .unwrap();
        library
            .annotate_package(package.id, &["source".into()])
            .unwrap();
        drop(library);
        images.detach(&mount);
        let offline = Library::open(&root).unwrap();
        assert!(!offline.packages().unwrap()[0].available);
        assert_eq!(
            offline.query(CatalogQuery::default()).unwrap().items[0].name,
            "RemountProbe"
        );
        assert!(offline.resolve_preview(file_id).is_err());
        drop(offline);
        images.attach(&filler, &filler_mount);
        images.attach(&image, &mount);
        let after = fs::metadata(&source).unwrap();
        let after_file = fs::metadata(source.join("A001.wav")).unwrap();
        assert_ne!(
            before.dev(),
            after.dev(),
            "the remount fixture must change the actual OS device number"
        );
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before_file.ino(), after_file.ino());
        eprintln!(
            "owned APFS remount: dev {} -> {}, root inode {}, file inode {}",
            before.dev(),
            after.dev(),
            after.ino(),
            after_file.ino()
        );
        let mut library = Library::open(&root).unwrap();
        assert!(
            library.packages().unwrap()[0].available,
            "the exact unchanged source must be available after remount"
        );
        library.relink_source(package.id, &source).unwrap();
        let moved = mount.join("MovedLibrarySource");
        fs::rename(&source, &moved).unwrap();
        assert!(!library.packages().unwrap()[0].available);
        library.relink_source(package.id, &moved).unwrap();
        let restored = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        assert_eq!(restored.id, asset.id);
        assert_eq!(restored.name, "RemountProbe");
        assert_eq!(restored.tags, ["laufwerk"]);
        assert_eq!(restored.files[0].id, file_id);
        assert_eq!(
            restored.files[0].relative_path,
            asset.files[0].relative_path
        );
        assert_eq!(library.packages().unwrap()[0].tags, ["source"]);
        assert_eq!(
            library.resolve_preview(file_id).unwrap(),
            moved.join("A001.wav").canonicalize().unwrap()
        );
        let exported = workspace.path("selected.wav");
        library.export(file_id, Some(&exported), false).unwrap();
        assert_eq!(fs::read(exported).unwrap(), original);
        assert_eq!(fs::read(moved.join("A001.wav")).unwrap(), original);
    }

    #[cfg(target_os = "macos")]
    fn legacy_catalog(root: &Path, source: &Path) {
        use std::os::unix::fs::MetadataExt;
        // Historical user_version=1 input to Library::open, not a query of its implementation.
        fs::create_dir(root).unwrap();
        let connection = Connection::open(root.join("catalog.sqlite3")).unwrap();
        connection.execute_batch("PRAGMA user_version = 1;
            CREATE TABLE packages (id INTEGER PRIMARY KEY, name TEXT NOT NULL, relative_path TEXT NOT NULL UNIQUE, source_path TEXT NOT NULL);
            CREATE TABLE assets (id INTEGER PRIMARY KEY, package_id INTEGER NOT NULL REFERENCES packages(id), name TEXT NOT NULL, original_folder TEXT NOT NULL);
            CREATE TABLE asset_files (id INTEGER PRIMARY KEY, asset_id INTEGER NOT NULL REFERENCES assets(id), original_filename TEXT NOT NULL, relative_path TEXT NOT NULL UNIQUE, format TEXT NOT NULL, size_bytes INTEGER NOT NULL);
            CREATE TABLE external_sources (package_id INTEGER PRIMARY KEY REFERENCES packages(id), root TEXT NOT NULL, identity TEXT NOT NULL);
            CREATE TABLE external_files (file_id INTEGER PRIMARY KEY REFERENCES asset_files(id), identity TEXT NOT NULL, modified TEXT NOT NULL);
            CREATE TABLE asset_tags (asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE, tag TEXT NOT NULL COLLATE NOCASE, PRIMARY KEY(asset_id, tag));
            CREATE TABLE package_tags (package_id INTEGER NOT NULL REFERENCES packages(id) ON DELETE CASCADE, tag TEXT NOT NULL COLLATE NOCASE, PRIMARY KEY(package_id, tag));
            INSERT INTO packages VALUES (3, 'Legacy Pack', 'external/legacy', 'original download');
            INSERT INTO assets VALUES (11, 3, 'Alter Fluss', 'Nature'), (12, 3, 'Alter Bach', 'Nature');
            INSERT INTO asset_tags VALUES (11, 'Wasser'), (12, 'Alt');
            INSERT INTO package_tags VALUES (3, 'Humble 2020');").unwrap();
        let metadata = fs::metadata(source).unwrap();
        connection
            .execute(
                "INSERT INTO external_sources VALUES (3, ?1, ?2)",
                params![
                    source.to_str().unwrap(),
                    format!("{}:{}", metadata.dev(), metadata.ino())
                ],
            )
            .unwrap();
        for (file_id, asset_id, filename, format) in
            [(19, 11, "river.wav", "WAV"), (20, 12, "stream.mp3", "MP3")]
        {
            let metadata = fs::metadata(source.join("Nature").join(filename)).unwrap();
            connection
                .execute(
                    "INSERT INTO asset_files VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        file_id,
                        asset_id,
                        filename,
                        format!("external/legacy/Nature/{filename}"),
                        format,
                        i64::try_from(metadata.len()).unwrap()
                    ],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO external_files VALUES (?1, ?2, ?3)",
                    params![
                        file_id,
                        format!("{}:{}", metadata.dev(), metadata.ino()),
                        format!("{:?}", metadata.modified().unwrap())
                    ],
                )
                .unwrap();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn opening_a_valid_legacy_catalog_preserves_annotations_and_access_after_remount() {
        use std::os::unix::fs::MetadataExt;
        let workspace = Workspace::new();
        let mut images = TestImages::default();
        let image = workspace.path("source.sparseimage");
        let filler = workspace.path("filler.sparseimage");
        let mount = workspace.path("mounted-source");
        TestImages::create(&image);
        TestImages::create(&filler);
        images.attach(&image, &mount);
        let source = mount.join("LegacySource");
        fs::create_dir_all(source.join("Nature")).unwrap();
        fs::write(source.join("Nature/river.wav"), b"legacy river bytes").unwrap();
        fs::write(source.join("Nature/stream.mp3"), b"legacy stream bytes").unwrap();
        let before = fs::metadata(&source).unwrap();
        let root = workspace.path("legacy-library");
        legacy_catalog(&root, &source);
        let library = Library::open(&root).unwrap();
        assert!(
            library.packages().unwrap()[0].available,
            "the exact valid legacy source must be migrated on open"
        );
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        let river = page.items.iter().find(|asset| asset.id == 11).unwrap();
        assert_eq!(river.name, "Alter Fluss");
        assert_eq!(river.tags, ["Wasser"]);
        assert_eq!(river.files[0].id, 19);
        assert_eq!(
            river.files[0].relative_path,
            "external/legacy/Nature/river.wav"
        );
        assert!(river.files[0].available);
        assert_eq!(library.packages().unwrap()[0].tags, ["Humble 2020"]);
        drop(library);
        images.detach(&mount);
        images.attach(&filler, &workspace.path("mounted-filler"));
        images.attach(&image, &mount);
        let after = fs::metadata(&source).unwrap();
        assert_ne!(before.dev(), after.dev());
        assert_eq!(before.ino(), after.ino());
        eprintln!(
            "migrated legacy APFS remount: dev {} -> {}, root inode {}",
            before.dev(),
            after.dev(),
            after.ino()
        );
        let mut library = Library::open(&root).unwrap();
        assert!(library.packages().unwrap()[0].available);
        library.relink_source(3, &source).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        let river = page.items.iter().find(|asset| asset.id == 11).unwrap();
        assert_eq!(river.name, "Alter Fluss");
        assert_eq!(river.tags, ["Wasser"]);
        assert_eq!(river.files[0].id, 19);
        assert_eq!(
            library.packages().unwrap()[0].source_path,
            "original download"
        );
        let exported = workspace.path("restored.wav");
        library.export(19, Some(&exported), false).unwrap();
        assert_eq!(fs::read(exported).unwrap(), b"legacy river bytes");
        assert_eq!(
            fs::read(library.resolve_preview(20).unwrap()).unwrap(),
            b"legacy stream bytes"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unavailable_legacy_sources_keep_annotations_and_reject_a_replacement_root() {
        let workspace = Workspace::new();
        let source = workspace.path("LegacySource");
        fs::create_dir_all(source.join("Nature")).unwrap();
        fs::write(source.join("Nature/river.wav"), b"legacy river bytes").unwrap();
        fs::write(source.join("Nature/stream.mp3"), b"legacy stream bytes").unwrap();
        let root = workspace.path("legacy-library");
        legacy_catalog(&root, &source);
        let parked = workspace.path("ParkedOriginal");
        fs::rename(&source, &parked).unwrap();
        let library = Library::open(&root).unwrap();
        assert!(!library.packages().unwrap()[0].available);
        assert_eq!(library.packages().unwrap()[0].tags, ["Humble 2020"]);
        assert!(library.resolve_preview(19).is_err());
        assert_eq!(
            library
                .query(CatalogQuery {
                    text: "Wasser".into(),
                    ..Default::default()
                })
                .unwrap()
                .items[0]
                .name,
            "Alter Fluss"
        );
        drop(library);
        fs::create_dir_all(source.join("Nature")).unwrap();
        fs::write(source.join("Nature/river.wav"), b"legacy river bytes").unwrap();
        fs::write(source.join("Nature/stream.mp3"), b"legacy stream bytes").unwrap();
        let mut library = Library::open(&root).unwrap();
        assert!(!library.packages().unwrap()[0].available);
        assert!(library.relink_source(3, &source).is_err());
        let export = workspace.path("game.wav");
        fs::write(&export, b"existing game sound").unwrap();
        assert!(library.export(19, Some(&export), true).is_err());
        assert_eq!(fs::read(export).unwrap(), b"existing game sound");
        drop(library);
        fs::remove_dir_all(&source).unwrap();
        fs::rename(&parked, &source).unwrap();
        let library = Library::open(&root).unwrap();
        assert!(library.packages().unwrap()[0].available);
        let river = library
            .query(CatalogQuery {
                text: "Wasser".into(),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        assert_eq!(river.id, 11);
        assert_eq!(river.name, "Alter Fluss");
        assert_eq!(river.tags, ["Wasser"]);
        assert_eq!(river.files[0].id, 19);
        assert!(river.files[0].available);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn legacy_migration_rejects_modified_replaced_or_linked_files_without_partial_conversion() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let workspace = Workspace::new();
        let source = workspace.path("LegacySource");
        fs::create_dir_all(source.join("Nature")).unwrap();
        let river = source.join("Nature/river.wav");
        let stream = source.join("Nature/stream.mp3");
        fs::write(&river, b"legacy river bytes").unwrap();
        fs::write(&stream, b"legacy stream bytes").unwrap();
        let modified = fs::metadata(&river).unwrap().modified().unwrap();
        let root = workspace.path("legacy-library");
        legacy_catalog(&root, &source);
        let original_stream = workspace.path("original-stream.mp3");
        fs::rename(&stream, &original_stream).unwrap();
        fs::write(&stream, b"legacy stream bytes").unwrap();
        let library = Library::open(&root).unwrap();
        assert!(!library.packages().unwrap()[0].available);
        assert!(
            library.resolve_preview(19).is_err(),
            "one changed file must prevent partially migrating its valid companion"
        );
        assert!(library.resolve_preview(20).is_err());
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 2);
        drop(library);
        fs::remove_file(&stream).unwrap();
        fs::rename(&original_stream, &stream).unwrap();
        let file = fs::File::options().write(true).open(&river).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(modified + Duration::from_secs(60)))
            .unwrap();
        drop(file);
        let library = Library::open(&root).unwrap();
        assert!(!library.packages().unwrap()[0].available);
        assert!(library.resolve_preview(20).is_err());
        drop(library);
        fs::File::options()
            .write(true)
            .open(&river)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let moved = source.join("MovedNature");
        fs::rename(source.join("Nature"), &moved).unwrap();
        symlink(&moved, source.join("Nature")).unwrap();
        let library = Library::open(&root).unwrap();
        assert!(!library.packages().unwrap()[0].available);
        assert!(library.resolve_preview(19).is_err());
        assert!(library.resolve_preview(20).is_err());
        drop(library);
        fs::remove_file(source.join("Nature")).unwrap();
        fs::rename(&moved, source.join("Nature")).unwrap();
        fs::set_permissions(&stream, fs::Permissions::from_mode(0o000)).unwrap();
        let mut library = Library::open(&root).unwrap();
        assert!(
            library.packages().unwrap()[0].available,
            "migration must use metadata only, even when contents cannot be opened"
        );
        library.relink_source(3, &source).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        let river_asset = page.items.iter().find(|asset| asset.id == 11).unwrap();
        assert_eq!(river_asset.name, "Alter Fluss");
        assert_eq!(river_asset.tags, ["Wasser"]);
        assert!(river_asset.files[0].available);
        assert_eq!(
            page.items.iter().find(|asset| asset.id == 12).unwrap().tags,
            ["Alt"]
        );
        assert!(library.resolve_preview(20).is_ok());
        assert_eq!(library.packages().unwrap()[0].tags, ["Humble 2020"]);
        fs::set_permissions(&stream, fs::Permissions::from_mode(0o600)).unwrap();
        let exported = workspace.path("restored.mp3");
        library.export(20, Some(&exported), false).unwrap();
        assert_eq!(fs::read(exported).unwrap(), b"legacy stream bytes");
    }

    #[test]
    fn matching_managed_files_are_variants_but_other_folders_and_packages_stay_separate() {
        let workspace = Workspace::new();
        let source = workspace.path("Variant Pack");
        fs::create_dir_all(source.join("ambience")).unwrap();
        fs::create_dir_all(source.join("effects")).unwrap();
        for extension in ["wav", "mp3", "ogg", "flac"] {
            fs::write(
                source.join(format!("ambience/River.{extension}")),
                extension,
            )
            .unwrap();
        }
        fs::write(source.join("ambience/Stream.mp3"), b"another sound").unwrap();
        fs::write(source.join("effects/River.wav"), b"different river").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let package = library.import_folder(&source).unwrap();
        assert_eq!(package.asset_count, 3);
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.items.len(), 3);
        let river = page
            .items
            .iter()
            .find(|asset| asset.name == "River" && asset.original_folder == "ambience")
            .unwrap();
        assert_eq!(river.files.len(), 4);
        assert_eq!(
            page.items
                .iter()
                .filter(|asset| asset.name == "River")
                .count(),
            2
        );
        for file in &river.files {
            assert_eq!(
                fs::read(library.resolve_preview(file.id).unwrap()).unwrap(),
                file.format.to_lowercase().as_bytes()
            );
        }
        let mp3_page = library
            .query(CatalogQuery {
                format: Some("MP3".into()),
                limit: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(mp3_page.total, 2);
        assert_eq!(
            mp3_page.items[0].files.len(),
            4,
            "filters select assets and retain every variant"
        );
        let next = library
            .query(CatalogQuery {
                format: Some("MP3".into()),
                limit: 1,
                offset: 1,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(next.items[0].name, "Stream");
        assert_ne!(next.items[0].id, mp3_page.items[0].id);
        let second = library.import_folder(&source).unwrap();
        assert_eq!(second.asset_count, 3);
        assert_ne!(second.id, package.id);
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 6);
        assert_eq!(library.packages().unwrap()[0].asset_count, 3);
    }

    #[test]
    fn zip_and_external_sources_apply_the_same_variant_groups_without_changing_sources() {
        use std::io::Write;
        let workspace = Workspace::new();
        let source = workspace.path("External Variants");
        fs::create_dir_all(source.join("water")).unwrap();
        fs::create_dir_all(source.join("effects")).unwrap();
        let entries: &[(&str, &[u8])] = &[
            ("water/River.wav", b"wave representation"),
            ("water/River.mp3", b"mp3 representation"),
            ("water/River.ogg", b"ogg representation"),
            ("water/River.flac", b"flac representation"),
            ("effects/River.wav", b"unrelated effect"),
            ("LICENSE.txt", b"companion remains"),
        ];
        let archive = workspace.path("Zip Variants.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for (path, bytes) in entries {
            fs::write(source.join(path), bytes).unwrap();
            zip.start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let archive_bytes = fs::read(&archive).unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let managed = library.import_zip(&archive).unwrap();
        let external = library.reference_folder(&source).unwrap();
        assert_eq!(managed.asset_count, 2);
        assert_eq!(external.asset_count, 2);
        assert!(!root.join(&external.relative_path).exists());
        assert_eq!(
            fs::read(root.join(&managed.relative_path).join("LICENSE.txt")).unwrap(),
            b"companion remains"
        );
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 4);
        for package_id in [managed.id, external.id] {
            let asset = page
                .items
                .iter()
                .find(|asset| asset.package_id == package_id && asset.original_folder == "water")
                .unwrap();
            assert_eq!(asset.files.len(), 4);
            let mp3 = asset
                .files
                .iter()
                .find(|file| file.format == "MP3")
                .unwrap();
            assert_eq!(mp3.original_folder, "water");
            let exported = workspace.path(&format!("selected-{package_id}.mp3"));
            library.export(mp3.id, Some(&exported), false).unwrap();
            assert_eq!(fs::read(exported).unwrap(), b"mp3 representation");
        }
        drop(library);
        let reopened = Library::open(&root)
            .unwrap()
            .query(CatalogQuery::default())
            .unwrap();
        assert_eq!(reopened.total, 4);
        assert_eq!(
            reopened
                .items
                .iter()
                .filter(|asset| asset.files.len() == 4)
                .count(),
            2
        );
        for (path, bytes) in entries {
            assert_eq!(fs::read(source.join(path)).unwrap(), *bytes);
        }
        assert_eq!(fs::read(archive).unwrap(), archive_bytes);
    }

    #[test]
    fn splitting_a_variant_retains_annotations_files_and_the_correction_after_reopen() {
        let workspace = Workspace::new();
        let source = workspace.path("Split Variants");
        fs::create_dir_all(source.join("water")).unwrap();
        let original = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        fs::write(source.join("water/River.mp3"), original).unwrap();
        fs::write(source.join("water/River.wav"), b"wave variant").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.import_folder(&source).unwrap();
        let asset = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        library
            .annotate_asset(asset.id, "Ruhiger Bach", &["water".into()])
            .unwrap();
        library
            .annotate_package(package.id, &["humble".into()])
            .unwrap();
        let selected = asset
            .files
            .iter()
            .find(|file| file.format == "MP3")
            .unwrap();
        library.analyze_audio(selected.id).unwrap();
        let preview_before = library.resolve_preview(selected.id).unwrap();
        let split_id = library.split_variant(selected.id).unwrap();
        assert_ne!(split_id, asset.id);
        assert!(
            library.split_variant(selected.id).is_err(),
            "a single file is already its own asset"
        );
        assert!(library.split_variant(-1).is_err());
        drop(library);
        let library = Library::open(&root).unwrap();
        let page = library
            .query(CatalogQuery {
                text: "humble".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            page.total, 2,
            "opening must not automatically undo a manual split"
        );
        assert!(page.items.iter().all(|item| item.name == "Ruhiger Bach"
            && item.tags == ["water"]
            && item.files.len() == 1));
        let split = page.items.iter().find(|item| item.id == split_id).unwrap();
        let file = &split.files[0];
        assert_eq!(file.id, selected.id);
        assert_eq!(file.relative_path, selected.relative_path);
        assert_eq!(file.original_filename, "River.mp3");
        assert_eq!(file.original_folder, "water");
        assert_eq!(file.audio.as_ref().unwrap().sample_rate, Some(44_100));
        assert_eq!(library.resolve_preview(file.id).unwrap(), preview_before);
        let exported = workspace.path("project-selected.mp3");
        library.export(file.id, Some(&exported), false).unwrap();
        assert_eq!(fs::read(exported).unwrap(), original);
        assert_eq!(fs::read(source.join("water/River.mp3")).unwrap(), original);
        assert_eq!(library.packages().unwrap()[0].asset_count, 2);
    }

    #[test]
    fn reassignment_retains_file_provenance_and_prevents_silent_loss_of_the_last_assets_name() {
        let workspace = Workspace::new();
        let source = workspace.path("Reassigned Source");
        fs::create_dir_all(source.join("ambience")).unwrap();
        fs::create_dir_all(source.join("effects")).unwrap();
        let mp3 = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        fs::write(source.join("ambience/River.mp3"), mp3).unwrap();
        fs::write(source.join("ambience/River.wav"), b"wave representation").unwrap();
        fs::write(source.join("effects/Impact.ogg"), b"unrelated effect").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.reference_folder(&source).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        let river = page
            .items
            .iter()
            .find(|asset| asset.name == "River")
            .unwrap();
        let target = page
            .items
            .iter()
            .find(|asset| asset.name == "Impact")
            .unwrap();
        library
            .annotate_asset(river.id, "Ruhiger Bach", &["water".into()])
            .unwrap();
        library
            .annotate_asset(target.id, "Starker Aufprall", &["impact".into()])
            .unwrap();
        let selected = river
            .files
            .iter()
            .find(|file| file.format == "MP3")
            .unwrap();
        library.analyze_audio(selected.id).unwrap();
        let original_path = library.resolve_preview(selected.id).unwrap();
        library.assign_variant(selected.id, target.id).unwrap();
        let matching_folder = library
            .query(CatalogQuery {
                original_folder: Some("ambience".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            matching_folder.total, 2,
            "original-folder filters must follow files across corrections"
        );
        let reassigned = matching_folder
            .items
            .iter()
            .find(|asset| asset.id == target.id)
            .unwrap();
        assert_eq!(reassigned.files.len(), 2);
        assert_eq!(reassigned.tags, ["impact", "water"]);
        let file = reassigned
            .files
            .iter()
            .find(|file| file.id == selected.id)
            .unwrap();
        assert_eq!(file.original_folder, "ambience");
        assert_eq!(file.relative_path, selected.relative_path);
        assert_eq!(file.original_filename, "River.mp3");
        assert_eq!(file.audio.as_ref().unwrap().sample_rate, Some(44_100));
        assert_eq!(
            library
                .query(CatalogQuery {
                    text: "ambience".into(),
                    ..Default::default()
                })
                .unwrap()
                .total,
            2
        );
        assert_eq!(
            library.catalog_filters(Some(package.id)).unwrap().folders,
            ["ambience", "effects"]
        );
        let last = river
            .files
            .iter()
            .find(|file| file.format == "WAV")
            .unwrap();
        let error = library.assign_variant(last.id, target.id).unwrap_err();
        assert!(
            error.contains("Ruhiger Bach") && error.contains("Starker Aufprall"),
            "a conflicting manual name needs an explicit correction: {error}"
        );
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 2);
        let retained = library
            .query(CatalogQuery {
                text: "Ruhiger Bach".into(),
                ..Default::default()
            })
            .unwrap()
            .items
            .remove(0);
        assert_eq!(retained.files[0].id, last.id);
        assert_eq!(retained.tags, ["water"]);
        library
            .annotate_asset(
                target.id,
                "Ruhiger Bach",
                &["impact".into(), "water".into()],
            )
            .unwrap();
        library.assign_variant(last.id, target.id).unwrap();
        assert!(library.assign_variant(selected.id, target.id).is_err());
        assert!(library.assign_variant(-1, target.id).is_err());
        assert!(library.assign_variant(selected.id, -1).is_err());
        let other = library.import_folder(&source).unwrap();
        let other_id = library
            .query(CatalogQuery {
                package_id: Some(other.id),
                ..Default::default()
            })
            .unwrap()
            .items[0]
            .id;
        assert!(library
            .assign_variant(selected.id, other_id)
            .unwrap_err()
            .contains("Paket"));
        drop(library);
        let library = Library::open(&root).unwrap();
        let page = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, target.id);
        assert_eq!(page.items[0].name, "Ruhiger Bach");
        assert_eq!(page.items[0].tags, ["impact", "water"]);
        assert_eq!(page.items[0].files.len(), 3);
        assert_eq!(library.resolve_preview(selected.id).unwrap(), original_path);
        let export = workspace.path("project-variant.mp3");
        library.export(selected.id, Some(&export), false).unwrap();
        assert_eq!(fs::read(export).unwrap(), mp3);
        assert_eq!(fs::read(source.join("ambience/River.mp3")).unwrap(), mp3);
        assert!(!root.join(&package.relative_path).exists());
    }

    #[test]
    fn assigning_the_last_named_variant_carries_its_name_and_tags_to_an_unnamed_target() {
        let workspace = Workspace::new();
        let source = workspace.path("Name Transfer");
        fs::create_dir_all(source.join("water")).unwrap();
        fs::create_dir_all(source.join("other")).unwrap();
        fs::write(source.join("water/River.wav"), b"named source").unwrap();
        fs::write(source.join("other/Stream.mp3"), b"default target").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        library.import_folder(&source).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        let river = page
            .items
            .iter()
            .find(|asset| asset.name == "River")
            .unwrap();
        let stream = page
            .items
            .iter()
            .find(|asset| asset.name == "Stream")
            .unwrap();
        library
            .annotate_asset(river.id, "Ruhiger Bach", &["water".into()])
            .unwrap();
        library
            .annotate_asset(stream.id, "Stream", &["stream".into()])
            .unwrap();
        library
            .assign_variant(river.files[0].id, stream.id)
            .unwrap();
        drop(library);
        let library = Library::open(&root).unwrap();
        let after = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(after.total, 1);
        assert_eq!(after.items[0].name, "Ruhiger Bach");
        assert_eq!(after.items[0].tags, ["stream", "water"]);
        assert_eq!(after.items[0].files.len(), 2);
        assert_eq!(
            library.catalog_filters(None).unwrap().folders,
            ["other", "water"]
        );
        for asset in page.items {
            let file = &asset.files[0];
            let retained = after.items[0]
                .files
                .iter()
                .find(|item| item.id == file.id)
                .unwrap();
            assert_eq!(retained.relative_path, file.relative_path);
            assert_eq!(retained.original_filename, file.original_filename);
            assert_eq!(
                library.resolve_preview(file.id).unwrap(),
                root.join(&file.relative_path).canonicalize().unwrap()
            );
        }
    }

    #[test]
    fn reassigning_cannot_silently_exceed_the_existing_annotation_limit() {
        let workspace = Workspace::new();
        let source = workspace.path("Tag Merge");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("River.wav"), b"named source").unwrap();
        fs::write(source.join("Stream.mp3"), b"default target").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let before = library.query(CatalogQuery::default()).unwrap();
        let river = before
            .items
            .iter()
            .find(|asset| asset.name == "River")
            .unwrap();
        let target = before
            .items
            .iter()
            .find(|asset| asset.name == "Stream")
            .unwrap();
        let source_tags: Vec<String> = (0..27).map(|index| format!("water-{index:02}")).collect();
        let target_tags: Vec<String> = (0..9).map(|index| format!("stream-{index:02}")).collect();
        library
            .annotate_asset(river.id, "Ruhiger Bach", &source_tags)
            .unwrap();
        library
            .annotate_asset(target.id, "Stream", &target_tags)
            .unwrap();
        assert!(library
            .assign_variant(river.files[0].id, target.id)
            .unwrap_err()
            .contains("32"));
        let unchanged = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(unchanged.total, 2);
        let kept_source = unchanged
            .items
            .iter()
            .find(|asset| asset.id == river.id)
            .unwrap();
        assert_eq!(kept_source.name, "Ruhiger Bach");
        assert_eq!(kept_source.tags, source_tags);
        assert_eq!(kept_source.files[0].id, river.files[0].id);
        let kept_target = unchanged
            .items
            .iter()
            .find(|asset| asset.id == target.id)
            .unwrap();
        assert_eq!(kept_target.name, "Stream");
        assert_eq!(kept_target.tags, target_tags);
        library
            .annotate_asset(target.id, "Stream", &target_tags[..5])
            .unwrap();
        library
            .assign_variant(river.files[0].id, target.id)
            .unwrap();
        let after = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(after.total, 1);
        assert_eq!(after.items[0].name, "Ruhiger Bach");
        assert_eq!(after.items[0].tags.len(), 32);
        assert!(source_tags
            .iter()
            .all(|tag| after.items[0].tags.contains(tag)));
        library
            .annotate_asset(target.id, "Bearbeitbarer Bach", &after.items[0].tags)
            .unwrap();
    }

    #[test]
    fn requested_audio_analysis_retains_original_properties_after_reopen() {
        let workspace = Workspace::new();
        let source = workspace.path("Mono Pack");
        fs::create_dir(&source).unwrap();
        let mut wav = Vec::new();
        let data_size = 22_050_u32 * 2 * 2;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&22_050_u32.to_le_bytes());
        wav.extend_from_slice(&44_100_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.resize(44 + data_size as usize, 0);
        fs::write(source.join("A001.wav"), &wav).unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        library.import_folder(&source).unwrap();
        let file = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0)
            .files
            .remove(0);
        let metadata = library.analyze_audio(file.id).unwrap();
        assert_eq!(metadata.status, AudioStatus::Ready);
        assert_eq!(metadata.sample_rate, Some(22_050));
        assert_eq!(metadata.channels, Some(1));
        assert_eq!(metadata.duration_seconds, Some(2.0));
        assert_eq!(file.size_bytes, 88_244);
        drop(library);
        let library = Library::open(&root).unwrap();
        let file = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0)
            .files
            .remove(0);
        let stored = file.audio.unwrap();
        assert_eq!(stored.sample_rate, Some(22_050));
        assert_eq!(stored.channels, Some(1));
        assert_eq!(stored.duration_seconds, Some(2.0));
        assert_eq!(fs::read(source.join("A001.wav")).unwrap(), wav);
    }

    #[test]
    fn all_target_codec_candidates_can_be_analyzed_without_changing_originals() {
        let workspace = Workspace::new();
        let source = workspace.path("Codec Pack");
        fs::create_dir(&source).unwrap();
        let fixtures: &[(&str, &[u8], &str, u32, f64)] = &[
            (
                "stereo44100.mp3",
                include_bytes!("../tests/fixtures/audio/stereo44100.mp3"),
                "MP3",
                44_100,
                0.25,
            ),
            (
                "stereo32000.ogg",
                include_bytes!("../tests/fixtures/audio/stereo32000.ogg"),
                "OGG",
                32_000,
                0.25,
            ),
            (
                "stereo48000.flac",
                include_bytes!("../tests/fixtures/audio/stereo48000.flac"),
                "FLAC",
                48_000,
                0.25,
            ),
        ];
        for (name, bytes, _, _, _) in fixtures {
            fs::write(source.join(name), bytes).unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 3);
        for (name, bytes, format, sample_rate, duration) in fixtures {
            let file = page
                .items
                .iter()
                .flat_map(|asset| &asset.files)
                .find(|file| file.original_filename == *name)
                .unwrap();
            assert_eq!(file.format, *format);
            assert_eq!(file.size_bytes, bytes.len() as i64);
            assert!(file.audio.is_none(), "discovery should not analyze files");
            let metadata = library.analyze_audio(file.id).unwrap();
            assert_eq!(
                metadata.status,
                AudioStatus::Ready,
                "{name}: {:?}",
                metadata.error
            );
            assert_eq!(metadata.sample_rate, Some(*sample_rate));
            assert_eq!(metadata.channels, Some(2));
            let observed_duration = metadata
                .duration_seconds
                .expect("readable fixture duration");
            assert!(
                (observed_duration - duration).abs() < 0.001,
                "{name}: {observed_duration}"
            );
            assert_eq!(fs::read(source.join(name)).unwrap(), *bytes);
        }
    }

    #[test]
    fn corrupt_and_unsupported_candidates_stay_identifiable_with_persisted_errors() {
        let workspace = Workspace::new();
        let source = workspace.path("Unreadable Pack");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Corrupt.wav"), b"not a wave file").unwrap();
        fs::write(
            source.join("NotVorbis.ogg"),
            include_bytes!("../tests/fixtures/audio/unsupported-ogg-flac.ogg"),
        )
        .unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        library.import_folder(&source).unwrap();
        let before = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(before.total, 2);
        for asset in &before.items {
            let metadata = library.analyze_audio(asset.files[0].id).unwrap();
            assert_eq!(metadata.status, AudioStatus::Error, "{}", asset.name);
            assert!(metadata
                .error
                .as_ref()
                .is_some_and(|message| !message.is_empty()));
            assert!(metadata.sample_rate.is_none());
            assert!(metadata.duration_seconds.is_none());
            assert!(metadata.channels.is_none());
        }
        drop(library);
        let library = Library::open(&root).unwrap();
        let after = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(after.total, 2);
        for asset in &after.items {
            assert!(asset.files[0].size_bytes > 0);
            assert!(!asset.files[0].original_filename.is_empty());
            assert_eq!(
                asset.files[0].audio.as_ref().unwrap().status,
                AudioStatus::Error
            );
            assert!(library.resolve_preview(asset.files[0].id).is_ok());
        }
    }

    #[test]
    fn referenced_audio_is_analyzed_only_when_requested_and_exports_original_bytes() {
        let workspace = Workspace::new();
        let source = workspace.path("External Codec Pack");
        fs::create_dir(&source).unwrap();
        let mp3 = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        fs::write(source.join("Selected.mp3"), mp3).unwrap();
        let unread = source.join("Unrequested.wav");
        fs::write(&unread, b"do not inspect this candidate on discovery").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unread, fs::Permissions::from_mode(0o0)).unwrap();
            assert!(fs::File::open(&unread).is_err());
        }
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.reference_folder(&source).unwrap();
        assert_eq!(package.asset_count, 2);
        assert!(!root.join("packages").exists());
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        assert!(page
            .items
            .iter()
            .flat_map(|asset| &asset.files)
            .all(|file| file.audio.is_none()));
        let selected = page
            .items
            .iter()
            .find(|asset| asset.name == "Selected")
            .unwrap()
            .files[0]
            .id;
        let metadata = library.analyze_audio(selected).unwrap();
        assert_eq!(metadata.status, AudioStatus::Ready);
        assert_eq!(metadata.sample_rate, Some(44_100));
        assert_eq!(metadata.channels, Some(2));
        let page = library.query(CatalogQuery::default()).unwrap();
        assert!(page
            .items
            .iter()
            .find(|asset| asset.name == "Unrequested")
            .unwrap()
            .files[0]
            .audio
            .is_none());
        let exported = workspace.path("game-sound.mp3");
        library.export(selected, Some(&exported), false).unwrap();
        assert_eq!(fs::read(exported).unwrap(), mp3);
        assert_eq!(fs::read(source.join("Selected.mp3")).unwrap(), mp3);
        drop(library);
        let library = Library::open(&root).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        let selected = page
            .items
            .iter()
            .find(|asset| asset.name == "Selected")
            .unwrap();
        assert_eq!(
            selected.files[0].audio.as_ref().unwrap().sample_rate,
            Some(44_100)
        );
    }

    #[test]
    fn imports_and_references_ignore_macos_metadata_but_preserve_package_bytes() {
        use std::io::Write;
        let workspace = Workspace::new();
        let source = workspace.path("Cyberpunk");
        let original = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        let contents: &[(&str, &[u8])] = &[
            ("Cyberpunk/Cyber Pulse A.mp3", original),
            ("__MACOSX-music/Actual.mp3", original),
            ("Cyberpunk/._Cyber Pulse A.mp3", b"AppleDouble metadata"),
            (
                "__MACOSX/Cyberpunk/._Cyber Pulse A.mp3",
                b"AppleDouble metadata",
            ),
            ("__MACOSX/Cyberpunk/Ghost.wav", b"archive metadata"),
            ("Cyberpunk/._Sample.ogg", b"AppleDouble metadata"),
            ("Cyberpunk/._Sample.flac", b"AppleDouble metadata"),
            ("._Root.wav", b"AppleDouble metadata"),
            ("LICENSE.txt", b"original license"),
        ];
        let archive = workspace.path("Cyberpunk.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for (relative, bytes) in contents {
            let path = source.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
            zip.start_file(*relative, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let archive_bytes = fs::read(&archive).unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let packages = [
            library.import_folder(&source).unwrap(),
            library.import_zip(&archive).unwrap(),
            library.reference_folder(&source).unwrap(),
        ];
        for package in &packages {
            assert_eq!(package.asset_count, 2, "{}", package.source_kind);
            let page = library
                .query(CatalogQuery {
                    package_id: Some(package.id),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(page.total, 2);
            let sound = page
                .items
                .iter()
                .find(|asset| asset.name == "Cyber Pulse A")
                .unwrap();
            assert_eq!(sound.original_folder, "Cyberpunk");
            assert!(sound.files[0].audio.is_none());
            assert_eq!(
                library.analyze_audio(sound.files[0].id).unwrap().status,
                AudioStatus::Ready
            );
            let export = workspace.path(&format!("export-{}.mp3", package.id));
            library
                .export(sound.files[0].id, Some(&export), false)
                .unwrap();
            assert_eq!(fs::read(export).unwrap(), original);
            let package_root = if package.source_kind == "external" {
                source.clone()
            } else {
                root.join(&package.relative_path)
            };
            for (relative, bytes) in contents {
                assert_eq!(fs::read(package_root.join(relative)).unwrap(), *bytes);
            }
        }
        assert_eq!(fs::read(archive).unwrap(), archive_bytes);
        drop(library);
        assert_eq!(
            Library::open(&root)
                .unwrap()
                .query(CatalogQuery::default())
                .unwrap()
                .total,
            6
        );
    }

    #[test]
    fn reopening_repairs_legacy_metadata_records_with_backup_and_preserves_real_sounds() {
        let workspace = Workspace::new();
        let source = workspace.path("Cyberpunk");
        let original = include_bytes!("../tests/fixtures/audio/stereo44100.mp3");
        let sidecars = [
            "Cyberpunk/._Cyber Pulse A.mp3",
            "__MACOSX/Cyberpunk/._Cyber Pulse A.mp3",
        ];
        fs::create_dir_all(source.join("Cyberpunk")).unwrap();
        fs::create_dir_all(source.join("__MACOSX/Cyberpunk")).unwrap();
        fs::write(source.join("Cyberpunk/Cyber Pulse A.mp3"), original).unwrap();
        for path in sidecars {
            fs::write(source.join(path), b"AppleDouble metadata").unwrap();
        }
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let packages = [
            library.import_folder(&source).unwrap(),
            library.reference_folder(&source).unwrap(),
        ];
        let before = library.query(CatalogQuery::default()).unwrap();
        let mut kept = Vec::new();
        for asset in &before.items {
            library
                .annotate_asset(asset.id, "Annotated pulse", &["keep me".into()])
                .unwrap();
            library
                .annotate_package(asset.package_id, &["game".into()])
                .unwrap();
            library.analyze_audio(asset.files[0].id).unwrap();
            kept.push((asset.package_id, asset.id, asset.files[0].id));
        }
        drop(library);
        // Fixture setup recreates records produced by the old extension-only importer,
        // including a sidecar manually grouped with a real sound and cached errors.
        let legacy = Connection::open(root.join("catalog.sqlite3")).unwrap();
        legacy.pragma_update(None, "user_version", 1).unwrap();
        for package in &packages {
            let (_, real_asset, _) = kept.iter().find(|row| row.0 == package.id).unwrap();
            legacy.execute("INSERT INTO assets(package_id, name, original_folder) VALUES (?1, 'Annotated sidecar', '__MACOSX/Cyberpunk')", [package.id]).unwrap();
            let bogus_asset = legacy.last_insert_rowid();
            legacy
                .execute(
                    "INSERT INTO asset_tags(asset_id, tag) VALUES (?1, 'legacy note')",
                    [bogus_asset],
                )
                .unwrap();
            for (path, asset_id) in sidecars.into_iter().zip([*real_asset, bogus_asset]) {
                legacy.execute("INSERT INTO asset_files(asset_id, original_filename, relative_path, format, size_bytes) VALUES (?1, '._Cyber Pulse A.mp3', ?2, 'MP3', 20)", params![asset_id, format!("{}/{}", package.relative_path, path)]).unwrap();
                let file_id = legacy.last_insert_rowid();
                legacy.execute("INSERT INTO audio_metadata(file_id, error) VALUES (?1, 'no suitable format reader found')", [file_id]).unwrap();
                if package.source_kind == "external" {
                    legacy.execute("INSERT INTO external_files(file_id, identity, modified) VALUES (?1, 'legacy', 'legacy')", [file_id]).unwrap();
                }
            }
        }
        drop(legacy);
        let library = Library::open(&root).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        for (package_id, asset_id, file_id) in kept {
            let asset = page
                .items
                .iter()
                .find(|asset| asset.id == asset_id)
                .unwrap();
            assert_eq!(asset.name, "Annotated pulse");
            assert_eq!(asset.tags, ["keep me"]);
            assert_eq!(asset.files.len(), 1);
            assert_eq!(asset.files[0].id, file_id);
            assert_eq!(
                asset.files[0].audio.as_ref().unwrap().status,
                AudioStatus::Ready
            );
            assert_eq!(
                library.analyze_audio(file_id).unwrap().status,
                AudioStatus::Ready
            );
            let export = workspace.path(&format!("repaired-{package_id}.mp3"));
            library.export(file_id, Some(&export), false).unwrap();
            assert_eq!(fs::read(export).unwrap(), original);
        }
        for package in library.packages().unwrap() {
            assert_eq!(package.asset_count, 1);
            assert_eq!(package.tags, ["game"]);
            let package_root = if package.source_kind == "external" {
                source.clone()
            } else {
                root.join(package.relative_path)
            };
            for path in sidecars {
                assert_eq!(
                    fs::read(package_root.join(path)).unwrap(),
                    b"AppleDouble metadata"
                );
            }
        }
        assert!(library
            .catalog_filters(None)
            .unwrap()
            .folders
            .iter()
            .all(|folder| !folder.contains("__MACOSX")));
        let backups = || {
            fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("catalog-before-macos-metadata-")
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(backups().len(), 1);
        // The user can recover annotations on discarded metadata from the untouched backup.
        let backup =
            Connection::open_with_flags(&backups()[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            backup
                .query_row(
                    "SELECT COUNT(*) FROM asset_tags WHERE tag = 'legacy note'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        drop(library);
        assert_eq!(
            Library::open(&root)
                .unwrap()
                .query(CatalogQuery::default())
                .unwrap()
                .total,
            2
        );
        assert_eq!(backups().len(), 1);
    }

    #[test]
    fn zip_import_preserves_hierarchy_originals_and_exports_after_reopen() {
        use std::io::Write;
        let workspace = Workspace::new();
        let archive = workspace.path("Ambience Pack.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("Water/A001.wav", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"original audio bytes").unwrap();
        zip.start_file("LICENSE.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"original license").unwrap();
        zip.finish().unwrap();
        let source_bytes = fs::read(&archive).unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let estimate = library.estimate_zip(&archive).unwrap();
        assert_eq!(estimate.uncompressed_bytes, 36);
        assert!(estimate.required_bytes > 36);
        assert!(estimate.has_space);
        let imported = library.import_zip(&archive).unwrap();
        assert_eq!(imported.name, "Ambience Pack");
        assert_eq!(imported.asset_count, 1);
        assert_eq!(
            fs::read(root.join(imported.relative_path).join("LICENSE.txt")).unwrap(),
            b"original license"
        );
        assert_eq!(fs::read(&archive).unwrap(), source_bytes);
        drop(library);
        let library = Library::open(&root).unwrap();
        let page = library
            .query(CatalogQuery {
                text: "Water".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].original_folder, "Water");
        let export = workspace.path("sound.wav");
        library
            .export(page.items[0].files[0].id, Some(&export), false)
            .unwrap();
        assert_eq!(fs::read(export).unwrap(), b"original audio bytes");
    }

    #[test]
    fn cancelled_zip_import_reports_progress_and_keeps_completed_packages() {
        use std::io::Write;
        let workspace = Workspace::new();
        let source = workspace.path("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("existing.wav"), b"completed asset").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let archive = workspace.path("cancel.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("new.wav", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&vec![1_u8; 256 * 1024]).unwrap();
        zip.finish().unwrap();
        let control = ImportControl::default();
        let mut copied = 0;
        let error = library
            .import_zip_with_progress(&archive, &control, |progress| {
                copied = copied.max(progress.completed_bytes);
                if progress.completed_bytes > 0 {
                    control.cancel();
                }
            })
            .unwrap_err();
        assert!(error.contains("abgebrochen"));
        assert!(copied > 0);
        assert_eq!(library.packages().unwrap().len(), 1);
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(
            fs::read(library.resolve_preview(page.items[0].files[0].id).unwrap()).unwrap(),
            b"completed asset"
        );
        assert_eq!(
            fs::read_dir(workspace.path("library/packages"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn unsafe_corrupt_or_colliding_archives_do_not_publish_partial_packages() {
        use std::io::Write;
        let workspace = Workspace::new();
        let source = workspace.path("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("keep.wav"), b"existing audio").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        for (name, entry) in [
            ("traversal", "../escape.wav"),
            ("parent", "child/../escape.wav"),
            ("absolute", "/escape.wav"),
            ("backslash", "..\\escape.wav"),
        ] {
            let archive = workspace.path(&format!("{name}.zip"));
            let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
            zip.start_file(entry, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"unsafe").unwrap();
            zip.finish().unwrap();
            assert!(library.import_zip(&archive).is_err(), "accepted {entry}");
        }
        let linked = workspace.path("link.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&linked).unwrap());
        zip.add_symlink("outside", "../", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.start_file(
            "outside/escape.wav",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"unsafe").unwrap();
        zip.finish().unwrap();
        assert!(library.import_zip(&linked).is_err());
        let collision = workspace.path("collision.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&collision).unwrap());
        zip.start_file("folder", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"companion").unwrap();
        zip.start_file("folder/sound.wav", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"audio").unwrap();
        zip.finish().unwrap();
        assert!(library.import_zip(&collision).is_err());
        let corrupt = workspace.path("corrupt.zip");
        fs::write(&corrupt, b"invalid ZIP input").unwrap();
        assert!(library.import_zip(&corrupt).is_err());
        assert_eq!(library.packages().unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(workspace.path("library/packages"))
                .unwrap()
                .count(),
            1
        );
        assert!(!workspace.path("escape.wav").exists());
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(
            fs::read(library.resolve_preview(page.items[0].files[0].id).unwrap()).unwrap(),
            b"existing audio"
        );
    }

    #[test]
    fn forced_zip64_entries_and_folder_cancellation_use_the_import_workflow() {
        use std::io::Write;
        let workspace = Workspace::new();
        let archive = workspace.path("ZIP64.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file(
            "nested/sound.wav",
            zip::write::SimpleFileOptions::default().large_file(true),
        )
        .unwrap();
        zip.write_all(b"forced ZIP64 audio").unwrap();
        zip.finish().unwrap();
        let bytes = fs::read(&archive).unwrap();
        // PKWARE local header: ZIP64 replaces both 32-bit size fields with sentinels.
        assert_eq!(&bytes[18..26], &[255_u8; 8]);
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let imported = library.import_zip(&archive).unwrap();
        assert_eq!(imported.asset_count, 1);
        let file = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
        assert_eq!(
            fs::read(library.resolve_preview(file).unwrap()).unwrap(),
            b"forced ZIP64 audio"
        );
        let folder = workspace.path("cancel-folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("other.wav"), vec![1_u8; 256 * 1024]).unwrap();
        let control = ImportControl::default();
        assert!(library
            .import_folder_with_progress(&folder, &control, |progress| {
                if progress.completed_bytes > 0 {
                    control.cancel();
                }
            })
            .is_err());
        assert_eq!(library.packages().unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(workspace.path("library/packages"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn declared_sizes_and_crc_errors_are_checked_before_publishing_assets() {
        use std::io::Write;
        let workspace = Workspace::new();
        let archive = workspace.path("declared.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file(
            "sound.wav",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(b"controlled audio bytes").unwrap();
        zip.finish().unwrap();
        let original = fs::read(&archive).unwrap();
        let central = original
            .windows(4)
            .position(|value| value == b"PK\x01\x02")
            .unwrap();
        let mut understated = original.clone();
        understated[central + 24..central + 28].copy_from_slice(&1_u32.to_le_bytes());
        fs::write(&archive, understated).unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        assert!(library.import_zip(&archive).is_err());
        let mut corrupt = original.clone();
        let name_length = u16::from_le_bytes([original[26], original[27]]) as usize;
        let extra_length = u16::from_le_bytes([original[28], original[29]]) as usize;
        corrupt[30 + name_length + extra_length] ^= 255;
        fs::write(&archive, corrupt).unwrap();
        assert!(library.import_zip(&archive).is_err());
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 0);
        assert_eq!(
            fs::read_dir(workspace.path("library/packages"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn zip_space_estimate_rejects_insufficient_storage_before_extracting() {
        use std::io::Write;
        let workspace = Workspace::new();
        let archive = workspace.path("space.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file(
            "sound.wav",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(b"small content").unwrap();
        zip.finish().unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let space = library.estimate_zip(&archive).unwrap().available_bytes;
        let mut bytes = fs::read(&archive).unwrap();
        let central = bytes
            .windows(4)
            .position(|value| value == b"PK\x01\x02")
            .unwrap();
        let name_length = u16::from_le_bytes([bytes[central + 28], bytes[central + 29]]) as usize;
        let end = bytes
            .windows(4)
            .position(|value| value == b"PK\x05\x06")
            .unwrap();
        let central_size = u32::from_le_bytes(bytes[end + 12..end + 16].try_into().unwrap());
        // A ZIP64 central-directory size field declares more content than this volume can hold.
        let declared = space.checked_mul(2).unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[central + 30..central + 32].copy_from_slice(&12_u16.to_le_bytes());
        let mut extra = vec![1_u8, 0, 8, 0];
        extra.extend_from_slice(&declared.to_le_bytes());
        bytes.splice(
            central + 46 + name_length..central + 46 + name_length,
            extra,
        );
        bytes[end + 12 + 12..end + 12 + 16].copy_from_slice(&(central_size + 12).to_le_bytes());
        fs::write(&archive, &bytes).unwrap();
        let estimate = library.estimate_zip(&archive).unwrap();
        assert_eq!(estimate.uncompressed_bytes, declared);
        assert!(!estimate.has_space);
        let error = library.import_zip(&archive).unwrap_err();
        assert!(error.contains("Nicht genug Speicherplatz"));
        assert_eq!(library.packages().unwrap().len(), 0);
        assert!(!workspace.path("library/packages").exists());
        assert_eq!(fs::read(archive).unwrap(), bytes);
    }

    #[test]
    fn annotations_survive_reopen_without_renaming_original_or_managed_files() {
        let workspace = Workspace::new();
        let source = workspace.path("80pixelartstyle");
        fs::create_dir_all(source.join("ambience")).unwrap();
        let original = source.join("ambience/A001.wav");
        fs::write(&original, b"unchanged original sound").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.import_folder(&source).unwrap();
        let before = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        let managed = library.resolve_preview(before.files[0].id).unwrap();
        library
            .annotate_asset(
                before.id,
                "Quiet river",
                &["water".into(), " ambience ".into(), "WATER".into()],
            )
            .unwrap();
        library
            .annotate_package(package.id, &["pixel art".into()])
            .unwrap();
        drop(library);
        let library = Library::open(&root).unwrap();
        let after = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        assert_eq!(after.name, "Quiet river");
        assert_eq!(after.tags, vec!["ambience", "water"]);
        assert_eq!(after.original_folder, "ambience");
        assert_eq!(after.files[0].original_filename, "A001.wav");
        assert_eq!(after.files[0].relative_path, before.files[0].relative_path);
        assert_eq!(library.packages().unwrap()[0].tags, vec!["pixel art"]);
        assert_eq!(library.resolve_preview(after.files[0].id).unwrap(), managed);
        assert_eq!(fs::read(original).unwrap(), b"unchanged original sound");
        assert_eq!(fs::read(&managed).unwrap(), b"unchanged original sound");
        drop(library);
        let mut library = Library::open(&root).unwrap();
        library
            .annotate_asset(after.id, "Rushing river", &["fast".into()])
            .unwrap();
        library.annotate_package(package.id, &[]).unwrap();
        assert!(library.annotate_asset(after.id, "", &[]).is_err());
        assert!(library
            .annotate_asset(after.id, "Lost name", &["x".repeat(65)])
            .is_err());
        assert!(library.annotate_asset(-1, "Missing", &[]).is_err());
        assert!(library.annotate_package(-1, &[]).is_err());
        let updated = library
            .query(CatalogQuery::default())
            .unwrap()
            .items
            .remove(0);
        assert_eq!(updated.name, "Rushing river");
        assert_eq!(updated.tags, vec!["fast"]);
        assert!(library.packages().unwrap()[0].tags.is_empty());
        assert_eq!(
            library.resolve_preview(updated.files[0].id).unwrap(),
            managed
        );
    }

    #[test]
    fn annotations_and_provenance_can_be_searched_with_combined_catalog_filters() {
        let workspace = Workspace::new();
        let source = workspace.path("80pixelartstyle");
        fs::create_dir_all(source.join("ambience")).unwrap();
        fs::create_dir_all(source.join("effects")).unwrap();
        fs::write(source.join("ambience/A001.wav"), b"river").unwrap();
        fs::write(source.join("effects/A001.wav"), b"effect").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let package = library.import_folder(&source).unwrap();
        let other = workspace.path("Other Pack");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("A001.wav"), b"other").unwrap();
        library.import_folder(&other).unwrap();
        let asset = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                ..Default::default()
            })
            .unwrap()
            .items
            .into_iter()
            .find(|asset| asset.original_folder == "ambience")
            .unwrap();
        library
            .annotate_asset(asset.id, "Quiet river", &["water".into()])
            .unwrap();
        library
            .annotate_package(package.id, &["pixel art".into()])
            .unwrap();
        for (text, count) in [
            ("Quiet river", 1),
            ("water", 1),
            ("pixel art", 2),
            ("A001.wav", 3),
            ("80pixelartstyle", 2),
            ("ambience", 1),
        ] {
            assert_eq!(
                library
                    .query(CatalogQuery {
                        text: text.into(),
                        ..Default::default()
                    })
                    .unwrap()
                    .total,
                count,
                "search {text}"
            );
        }
        let page = library
            .query(CatalogQuery {
                package_id: Some(package.id),
                original_folder: Some("ambience".into()),
                format: Some("wav".into()),
                tag: Some("PIXEL ART".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, asset.id);
        assert_eq!(page.items[0].files[0].original_filename, "A001.wav");
        assert_eq!(page.items[0].original_folder, "ambience");
        for query in [
            CatalogQuery {
                original_folder: Some("".into()),
                ..Default::default()
            },
            CatalogQuery {
                tag: Some("water".into()),
                ..Default::default()
            },
        ] {
            assert_eq!(library.query(query).unwrap().total, 1);
        }
        for query in [
            CatalogQuery {
                original_folder: Some("missing".into()),
                ..Default::default()
            },
            CatalogQuery {
                format: Some("MP3".into()),
                ..Default::default()
            },
            CatalogQuery {
                tag: Some("missing".into()),
                ..Default::default()
            },
        ] {
            assert_eq!(library.query(query).unwrap().total, 0);
        }
        let filters = library.catalog_filters(Some(package.id)).unwrap();
        assert_eq!(filters.folders, vec!["ambience", "effects"]);
        assert_eq!(filters.formats, vec!["WAV"]);
        assert_eq!(filters.tags, vec!["pixel art", "water"]);
        assert_eq!(
            library.catalog_filters(None).unwrap().folders,
            vec!["", "ambience", "effects"]
        );
        // The absolute import origin is provenance, not searchable text.
        let downloads = workspace.path("Downloads/Plain");
        fs::create_dir_all(&downloads).unwrap();
        fs::write(downloads.join("B002.wav"), b"plain").unwrap();
        library.import_folder(&downloads).unwrap();
        let search = |text: &str| {
            library
                .query(CatalogQuery {
                    text: text.into(),
                    ..Default::default()
                })
                .unwrap()
                .total
        };
        assert_eq!(search("downloads"), 0);
        assert_eq!(search("plain"), 1);
    }

    #[test]
    fn opening_runs_one_time_repairs_once_and_removes_interrupted_imports() {
        let workspace = Workspace::new();
        let source = workspace.path("Pack");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("tone.wav"), b"tone").unwrap();
        let root = workspace.path("library");
        Library::create(&root)
            .unwrap()
            .import_folder(&source)
            .unwrap();
        let leftover = root.join("packages/.import-crashed");
        fs::create_dir_all(leftover.join("nested")).unwrap();
        fs::write(leftover.join("nested/partial.wav"), b"partial").unwrap();
        // A recorded leftover sidecar shows the repair scan is skipped once the marker is set.
        let catalog = Connection::open(root.join("catalog.sqlite3")).unwrap();
        assert_eq!(
            catalog
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        catalog.execute("INSERT INTO asset_files(asset_id, original_filename, relative_path, format, size_bytes) SELECT asset_id, '._tone.wav', relative_path || '.sidecar/._tone.wav', 'WAV', 4 FROM asset_files", []).unwrap();
        drop(catalog);
        let library = Library::open(&root).unwrap();
        assert!(!leftover.exists());
        assert!(root.join("packages").read_dir().unwrap().count() == 1);
        drop(library);
        let library = Library::writable(&root).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].files.len(), 2, "marker skips the rescan");
        assert!(fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("catalog-before-macos-metadata-")));
    }

    #[test]
    fn selected_and_default_libraries_can_be_reopened() {
        let workspace = Workspace::new();
        let selected = workspace.path("selected");
        let library = Library::create(&selected).unwrap();
        assert_eq!(library.root(), selected.canonicalize().unwrap());
        drop(library);
        assert_eq!(
            Library::open(&selected).unwrap().root(),
            selected.canonicalize().unwrap()
        );
        let home = workspace.path("home");
        fs::create_dir(&home).unwrap();
        let automatic = Library::create_default(&home).unwrap();
        assert_eq!(
            automatic.root(),
            home.join("Game Asset Library").canonicalize().unwrap()
        );
        drop(automatic);
        assert!(Library::open(&home.join("Game Asset Library")).is_ok());
    }
    #[test]
    fn referencing_a_folder_catalogs_provenance_without_copying_or_reading_contents() {
        let workspace = Workspace::new();
        let source = workspace.path("External Pack");
        fs::create_dir_all(source.join("Ambience/Rivers")).unwrap();
        let sound = source.join("Ambience/Rivers/a104.wav");
        fs::write(&sound, b"external sound bytes").unwrap();
        fs::write(source.join("LICENSE.txt"), b"keep this companion").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&sound, fs::Permissions::from_mode(0o000)).unwrap();
        }
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.reference_folder(&source).unwrap();
        assert_eq!(package.name, "External Pack");
        assert_eq!(package.asset_count, 1);
        assert_eq!(package.source_kind, "external");
        assert!(!root.join("packages").exists());
        drop(library);
        let library = Library::open(&root).unwrap();
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 1);
        let asset = &page.items[0];
        assert_eq!(asset.package_name, "External Pack");
        assert_eq!(asset.original_folder, "Ambience/Rivers");
        assert_eq!(asset.files[0].original_filename, "a104.wav");
        assert!(asset.files[0].available);
        assert_eq!(
            library.resolve_preview(asset.files[0].id).unwrap(),
            sound.canonicalize().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&sound, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(fs::read(&sound).unwrap(), b"external sound bytes");
        assert_eq!(
            fs::read(source.join("LICENSE.txt")).unwrap(),
            b"keep this companion"
        );
    }
    #[test]
    fn an_unavailable_external_source_can_be_relinked_without_recreating_assets() {
        let workspace = Workspace::new();
        let source = workspace.path("External Pack");
        fs::create_dir_all(source.join("Nature")).unwrap();
        fs::write(
            source.join("Nature/river.wav"),
            b"unchanged referenced sound",
        )
        .unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.reference_folder(&source).unwrap();
        let first = library.query(CatalogQuery::default()).unwrap();
        let asset_id = first.items[0].id;
        let file_id = first.items[0].files[0].id;
        library
            .annotate_asset(
                asset_id,
                "Reißender Fluss",
                &["Wasser".into(), "Ambience".into()],
            )
            .unwrap();
        library
            .annotate_package(package.id, &["Nature".into()])
            .unwrap();
        let destination = workspace.path("game.wav");
        library.export(file_id, Some(&destination), false).unwrap();
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"unchanged referenced sound"
        );
        let moved_source = workspace.path("Source At New Location");
        fs::rename(&source, &moved_source).unwrap();
        drop(library);
        let mut library = Library::open(&root).unwrap();
        let offline = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(offline.total, 1);
        assert_eq!(offline.items[0].id, asset_id);
        assert_eq!(offline.items[0].name, "Reißender Fluss");
        assert_eq!(offline.items[0].tags, ["Ambience", "Wasser"]);
        assert_eq!(library.packages().unwrap()[0].tags, ["Nature"]);
        assert!(!offline.items[0].files[0].available);
        assert!(!library.packages().unwrap()[0].available);
        assert!(library.resolve_preview(file_id).is_err());
        assert!(library.export(file_id, Some(&destination), true).is_err());
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"unchanged referenced sound"
        );
        let restored = library.relink_source(package.id, &moved_source).unwrap();
        assert_eq!(restored.id, package.id);
        assert_eq!(restored.source_path, package.source_path);
        assert_eq!(restored.tags, ["Nature"]);
        assert!(restored.available);
        assert_eq!(
            restored.external_root.as_deref(),
            Some(moved_source.canonicalize().unwrap().to_str().unwrap())
        );
        let available = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(available.items[0].id, asset_id);
        assert_eq!(available.items[0].name, "Reißender Fluss");
        assert_eq!(available.items[0].tags, ["Ambience", "Wasser"]);
        assert_eq!(available.items[0].files[0].id, file_id);
        assert!(available.items[0].files[0].available);
        library.export(file_id, Some(&destination), true).unwrap();
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"unchanged referenced sound"
        );
        drop(library);
        let moved_library = workspace.path("moved-library");
        fs::rename(&root, &moved_library).unwrap();
        let library = Library::open(&moved_library).unwrap();
        assert_eq!(
            library.query(CatalogQuery::default()).unwrap().items[0].id,
            asset_id
        );
        assert_eq!(
            fs::read(library.resolve_preview(file_id).unwrap()).unwrap(),
            b"unchanged referenced sound"
        );
    }
    #[test]
    fn external_sources_reject_substituted_files_and_cannot_be_export_destinations() {
        let workspace = Workspace::new();
        let source = workspace.path("external");
        fs::create_dir(&source).unwrap();
        let sound = source.join("sound.wav");
        fs::write(&sound, b"original source bytes").unwrap();
        let companion = source.join("keep.txt");
        fs::write(&companion, b"source companion").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let package = library.reference_folder(&source).unwrap();
        let file_id = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
        assert!(library.export(file_id, Some(&companion), true).is_err());
        assert_eq!(fs::read(&companion).unwrap(), b"source companion");
        let unrelated = workspace.path("unrelated");
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("sound.wav"), b"different sound data").unwrap();
        assert!(library.relink_source(package.id, &unrelated).is_err());
        assert_eq!(
            library.resolve_preview(file_id).unwrap(),
            sound.canonicalize().unwrap()
        );
        assert!(library
            .relink_source(package.id, &workspace.path("missing"))
            .is_err());
        fs::rename(&sound, source.join("old-sound.wav")).unwrap();
        fs::write(&sound, b"different sound data").unwrap();
        assert!(!library.query(CatalogQuery::default()).unwrap().items[0].files[0].available);
        assert!(library.resolve_preview(file_id).is_err());
        assert!(library.relink_source(package.id, &source).is_err());
        let destination = workspace.path("game.wav");
        fs::write(&destination, b"game work").unwrap();
        assert!(library.export(file_id, Some(&destination), true).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"game work");
    }
    #[test]
    fn referencing_discovers_all_target_audio_candidates_without_claiming_codec_support() {
        let workspace = Workspace::new();
        let source = workspace.path("mixed-formats");
        fs::create_dir(&source).unwrap();
        for filename in ["sound.wav", "sound.mp3", "sound.ogg", "sound.flac"] {
            fs::write(source.join(filename), b"unread audio candidate").unwrap();
        }
        fs::write(source.join("companion.txt"), b"companion").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let package = library.reference_folder(&source).unwrap();
        assert_eq!(package.asset_count, 1);
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.total, 1);
        let mut formats: Vec<_> = page
            .items
            .iter()
            .flat_map(|asset| &asset.files)
            .map(|file| file.format.as_str())
            .collect();
        formats.sort_unstable();
        assert_eq!(formats, ["FLAC", "MP3", "OGG", "WAV"]);
        assert!(page.items[0].files.iter().all(|file| file.audio.is_none()));
    }
    #[test]
    fn imported_folder_retains_provenance_companions_and_originals_after_reopen() {
        let workspace = Workspace::new();
        let source = workspace.path("River Pack");
        fs::create_dir_all(source.join("Ambience/Rivers")).unwrap();
        let wav = b"RIFF\x2c\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x08\0\0\0\0\0\0\0\0\0\0\0";
        fs::write(source.join("Ambience/Rivers/a104.wav"), wav).unwrap();
        fs::write(source.join("Ambience/LICENSE.txt"), b"Keep this license").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.import_folder(&source).unwrap();
        assert_eq!(package.name, "River Pack");
        assert_eq!(package.asset_count, 1);
        assert_eq!(
            fs::read(
                root.join(&package.relative_path)
                    .join("Ambience/LICENSE.txt")
            )
            .unwrap(),
            b"Keep this license"
        );
        assert_eq!(
            fs::read(source.join("Ambience/Rivers/a104.wav")).unwrap(),
            wav
        );
        drop(library);
        let library = Library::open(&root).unwrap();
        let page = library
            .query(CatalogQuery {
                text: "Rivers".into(),
                limit: 40,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        let asset = &page.items[0];
        assert_eq!(asset.package_name, "River Pack");
        assert_eq!(asset.original_folder, "Ambience/Rivers");
        assert_eq!(asset.files[0].original_filename, "a104.wav");
        assert_eq!(asset.files[0].format, "WAV");
        assert_eq!(
            fs::read(library.resolve_preview(asset.files[0].id).unwrap()).unwrap(),
            wav
        );
        assert_eq!(
            library
                .query(CatalogQuery {
                    text: "River Pack".into(),
                    limit: 40,
                    ..Default::default()
                })
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            library
                .query(CatalogQuery {
                    text: "a104.wav".into(),
                    limit: 40,
                    ..Default::default()
                })
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn export_preserves_bytes_and_requires_an_explicit_replacement_choice() {
        let workspace = Workspace::new();
        let source = workspace.path("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("sound.wav"), b"original sound bytes").unwrap();
        let mut library = Library::create(&workspace.path("library")).unwrap();
        library.import_folder(&source).unwrap();
        let file_id = library.query(CatalogQuery::default()).unwrap().items[0].files[0].id;
        let project = workspace.path("game");
        fs::create_dir(&project).unwrap();
        let destination = project.join("sound.wav");
        assert_eq!(
            library.export(file_id, None, false).unwrap(),
            ExportOutcome::Cancelled
        );
        assert!(!destination.exists());
        fs::write(&destination, b"existing project work").unwrap();
        assert_eq!(
            library.export(file_id, Some(&destination), false).unwrap(),
            ExportOutcome::ReplacementRequired
        );
        assert_eq!(fs::read(&destination).unwrap(), b"existing project work");
        assert!(matches!(
            library.export(file_id, Some(&destination), true).unwrap(),
            ExportOutcome::Exported { .. }
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"original sound bytes");
        assert_eq!(
            fs::read(library.resolve_preview(file_id).unwrap()).unwrap(),
            b"original sound bytes"
        );
        let fresh = project.join("fresh.wav");
        assert!(matches!(
            library.export(file_id, Some(&fresh), false).unwrap(),
            ExportOutcome::Exported { .. }
        ));
        assert_eq!(fs::read(&fresh).unwrap(), b"original sound bytes");
        let managed = library.resolve_preview(file_id).unwrap();
        assert!(library.export(file_id, Some(&managed), true).is_err());
        #[cfg(unix)]
        {
            let alias = project.join("alias.wav");
            fs::hard_link(&managed, &alias).unwrap();
            assert!(library.export(file_id, Some(&alias), true).is_err());
            assert_eq!(fs::read(&alias).unwrap(), b"original sound bytes");
        }
    }

    #[test]
    fn moved_library_retains_package_origin_and_resolves_managed_files() {
        let workspace = Workspace::new();
        let source = workspace.path("Original Pack");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("sound.wav"), b"portable original bytes").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.import_folder(&source).unwrap();
        assert_eq!(
            package.source_path,
            source.canonicalize().unwrap().to_str().unwrap()
        );
        drop(library);
        let moved = workspace.path("moved-library");
        fs::rename(&root, &moved).unwrap();
        let library = Library::open(&moved).unwrap();
        let packages = library.packages().unwrap();
        assert_eq!(
            packages[0].source_path,
            source.canonicalize().unwrap().to_str().unwrap()
        );
        let page = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(page.items.len(), 1);
        let resolved = library.resolve_preview(page.items[0].files[0].id).unwrap();
        assert!(resolved.starts_with(moved.canonicalize().unwrap()));
        assert_eq!(fs::read(resolved).unwrap(), b"portable original bytes");
        let exported = workspace.path("export.wav");
        library
            .export(page.items[0].files[0].id, Some(&exported), false)
            .unwrap();
        assert_eq!(fs::read(exported).unwrap(), b"portable original bytes");
    }

    #[test]
    fn file_and_storage_failures_leave_existing_packages_and_project_work_intact() {
        let workspace = Workspace::new();
        let source = workspace.path("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("sound.wav"), b"safe sound bytes").unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        library.import_folder(&source).unwrap();
        assert!(library
            .import_folder(&workspace.path("unavailable-source"))
            .is_err());
        assert_eq!(library.packages().unwrap().len(), 1);
        assert_eq!(library.query(CatalogQuery::default()).unwrap().total, 1);
        #[cfg(unix)]
        {
            let unsafe_source = workspace.path("linked-source");
            fs::create_dir(&unsafe_source).unwrap();
            fs::write(unsafe_source.join("another.wav"), b"other sound").unwrap();
            std::os::unix::fs::symlink(&source, unsafe_source.join("linked-folder")).unwrap();
            assert!(library.import_folder(&unsafe_source).is_err());
            assert_eq!(library.packages().unwrap().len(), 1);
        }
        let nonempty = workspace.path("nonempty");
        fs::create_dir(&nonempty).unwrap();
        fs::write(nonempty.join("keep.txt"), b"existing storage contents").unwrap();
        assert!(Library::create(&nonempty).is_err());
        assert_eq!(
            fs::read(nonempty.join("keep.txt")).unwrap(),
            b"existing storage contents"
        );
        let asset = &library.query(CatalogQuery::default()).unwrap().items[0];
        assert!(library
            .export(
                asset.files[0].id,
                Some(&workspace.path("missing-project/sound.wav")),
                false
            )
            .is_err());
        let destination = workspace.path("project-work.wav");
        fs::write(&destination, b"existing project work").unwrap();
        fs::remove_file(library.resolve_preview(asset.files[0].id).unwrap()).unwrap();
        assert!(library.resolve_preview(asset.files[0].id).is_err());
        assert!(library
            .export(asset.files[0].id, Some(&destination), true)
            .is_err());
        assert_eq!(fs::read(destination).unwrap(), b"existing project work");
        assert_eq!(
            fs::read(source.join("sound.wav")).unwrap(),
            b"safe sound bytes"
        );
    }
    #[test]
    fn large_catalog_keeps_filtered_pages_stable_and_recovers_an_invalid_page() {
        use std::io::Write;

        let workspace = Workspace::new();
        let archive = workspace.path("Humble Sound Pack.zip");
        let wav = b"RIFF\x2c\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x08\0\0\0\0\0\0\0\0\0\0\0";
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for index in (0..10_000).rev() {
            let folder = if index % 2 == 0 {
                "ambience"
            } else {
                "effects"
            };
            zip.start_file(
                format!("{folder}/Pulse_{index:05}.wav"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(wav).unwrap();
        }
        zip.finish().unwrap();
        let root = workspace.path("library");
        let mut library = Library::create(&root).unwrap();
        let package = library.import_zip(&archive).unwrap();
        assert_eq!(package.asset_count, 10_000);
        library
            .annotate_package(package.id, &["humble".into()])
            .unwrap();

        let first = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(first.total, 10_000);
        assert_eq!(first.items.len(), 40);
        assert_eq!(first.items[0].name, "Pulse_00000");
        assert_eq!(first.items[39].name, "Pulse_00039");
        assert_eq!(
            library
                .query(CatalogQuery {
                    limit: i64::MAX,
                    ..Default::default()
                })
                .unwrap()
                .items
                .len(),
            100
        );
        let second = library
            .query(CatalogQuery {
                offset: 40,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(second.items[0].name, "Pulse_00040");
        assert_eq!(second.items[39].name, "Pulse_00079");
        let last = library
            .query(CatalogQuery {
                offset: 9_960,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(last.items.len(), 40);
        assert_eq!(last.items[39].name, "Pulse_09999");

        let filtered = |offset| CatalogQuery {
            workflow: None,
            kind: None,
            text: "Pulse_09".into(),
            package_id: Some(package.id),
            asset_id: None,
            folder_id: None,
            original_folder: Some("ambience".into()),
            format: Some("wav".into()),
            tag: Some("HUMBLE".into()),
            offset,
            limit: 40,
        };
        let filtered_first = library.query(filtered(0)).unwrap();
        let filtered_second = library.query(filtered(40)).unwrap();
        assert_eq!(filtered_first.total, 500);
        assert_eq!(filtered_first.items.len(), 40);
        assert_eq!(filtered_first.items[0].name, "Pulse_09000");
        assert_eq!(filtered_first.items[39].name, "Pulse_09078");
        assert_eq!(filtered_second.total, 500);
        assert_eq!(filtered_second.items[0].name, "Pulse_09080");
        assert_eq!(filtered_second.items[39].name, "Pulse_09158");
        assert!(filtered_second
            .items
            .iter()
            .all(|asset| asset.package_id == package.id
                && asset.original_folder == "ambience"
                && asset.files.iter().all(|file| file.format == "WAV")));
        let filters = library.catalog_filters(Some(package.id)).unwrap();
        assert_eq!(filters.folders, ["ambience", "effects"]);
        assert_eq!(filters.formats, ["WAV"]);
        assert_eq!(filters.tags, ["humble"]);

        let asset_id = filtered_second.items[0].id;
        library
            .annotate_asset(asset_id, "River at night", &["favorite".into()])
            .unwrap();
        drop(library);
        let library = Library::open(&root).unwrap();
        let repeated = library
            .query(CatalogQuery {
                offset: 40,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            repeated
                .items
                .iter()
                .map(|asset| asset.id)
                .collect::<Vec<_>>(),
            second
                .items
                .iter()
                .map(|asset| asset.id)
                .collect::<Vec<_>>()
        );
        let narrowed = library
            .query(CatalogQuery {
                text: "River at night".into(),
                package_id: Some(package.id),
                original_folder: Some("ambience".into()),
                format: Some("WAV".into()),
                tag: Some("favorite".into()),
                offset: 9_960,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(narrowed.total, 1);
        assert_eq!(narrowed.offset, 0);
        assert_eq!(narrowed.items[0].id, asset_id);
        let missing = library
            .query(CatalogQuery {
                text: "no matching sound".into(),
                offset: 9_960,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(missing.total, 0);
        assert_eq!(missing.offset, 0);
        assert!(missing.items.is_empty());
    }

    #[test]
    fn catalog_results_are_bounded_and_packages_remain_distinguishable() {
        let workspace = Workspace::new();
        let source = workspace.path("many-sounds");
        fs::create_dir(&source).unwrap();
        for index in 0..45 {
            fs::write(source.join(format!("sound-{index:02}.wav")), b"sound").unwrap();
        }
        let mut library = Library::create(&workspace.path("library")).unwrap();
        let package = library.import_folder(&source).unwrap();
        let first = library.query(CatalogQuery::default()).unwrap();
        assert_eq!(first.total, 45);
        assert_eq!(first.items.len(), 40);
        assert_eq!(first.items[0].name, "sound-00");
        let last = library
            .query(CatalogQuery {
                offset: 40,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(last.items.len(), 5);
        assert_eq!(last.items[0].name, "sound-40");
        let filtered = library
            .query(CatalogQuery {
                text: "sound-04".into(),
                package_id: Some(package.id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.total, 1);
        assert_eq!(filtered.items[0].files[0].original_filename, "sound-04.wav");
        assert_eq!(
            library
                .query(CatalogQuery {
                    package_id: Some(package.id + 1),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
    }
}

#[cfg(test)]
#[path = "audio_population_tests.rs"]
mod audio_population_tests;
