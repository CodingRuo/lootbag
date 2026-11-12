use super::Result;
#[cfg(target_os = "macos")]
use super::{
    storage::{anchored, normal_path, required_text, CatalogReader, QueryBudget, Stamp},
    Library,
};
use std::path::Path;
#[cfg(target_os = "macos")]
use std::{
    ffi::OsStr,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TrashRemovalEntry {
    pub id: i64,
    pub package_id: i64,
    pub name: String,
    pub deleted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedTrashRemoval {
    pub token: String,
    pub package_name: String,
    pub source_path: String,
    pub external_root: String,
    pub deleted_at: i64,
    pub asset_count: i64,
    pub file_count: i64,
}

pub(crate) struct TrashRemovalPlan {
    pub reply: PreparedTrashRemoval,
    #[cfg(target_os = "macos")]
    entry: TrashRemovalEntry,
    #[cfg(target_os = "macos")]
    relative_path: String,
    #[cfg(target_os = "macos")]
    external_identity: String,
    #[cfg(target_os = "macos")]
    root_identity: (u64, u64),
    #[cfg(target_os = "macos")]
    catalog_identity: (u64, u64),
}

#[cfg(target_os = "macos")]
const WORK: Duration = Duration::from_secs(2);
#[cfg(target_os = "macos")]
const ROWS: i64 = 100_000;
const INELIGIBLE: &str =
    "Nur vollständig gelöschte externe Pakete können aus dem Katalog entfernt werden";

#[cfg(target_os = "macos")]
struct Record {
    name: String,
    source: String,
    path: String,
    external_root: String,
    external_identity: String,
    assets: i64,
    files: i64,
}

#[cfg(target_os = "macos")]
fn record(c: &rusqlite::Connection, entry: &TrashRemovalEntry) -> Result<Record> {
    let mut query = c.prepare("SELECT t.package_id,p.name,t.deleted_at,p.source_path,p.relative_path,s.root,s.identity,
        (SELECT count(*) FROM (SELECT 1 FROM assets WHERE package_id=p.id LIMIT 100001)),
        (SELECT count(*) FROM (SELECT 1 FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE a.package_id=p.id LIMIT 100001)),
        (SELECT count(*) FROM (SELECT 1 FROM packages WHERE relative_path=p.relative_path LIMIT 2)),
        EXISTS(SELECT 1 FROM asset_files f JOIN assets a ON a.id=f.asset_id LEFT JOIN external_files e ON e.file_id=f.id WHERE a.package_id=p.id AND (e.file_id IS NULL OR substr(f.relative_path,1,length(p.relative_path)+1) != p.relative_path || '/'))
        FROM trash t JOIN packages p ON p.id=t.package_id JOIN external_sources s ON s.package_id=p.id
        WHERE t.id=?1 AND t.file_id IS NULL LIMIT 2").map_err(|e| e.to_string())?;
    let mut rows = query.query([entry.id]).map_err(|e| e.to_string())?;
    let row = rows.next().map_err(|e| e.to_string())?.ok_or(INELIGIBLE)?;
    let mut bytes = 0;
    let name = required_text(row, 1, &mut bytes, 24_576)?;
    if row.get::<_, i64>(0).map_err(|e| e.to_string())? != entry.package_id
        || name != entry.name
        || row.get::<_, i64>(2).map_err(|e| e.to_string())? != entry.deleted_at
    {
        return Err("Papierkorbeintrag wurde verändert; bitte aktualisieren".into());
    }
    let result = Record {
        name,
        source: required_text(row, 3, &mut bytes, 24_576)?,
        path: required_text(row, 4, &mut bytes, 24_576)?,
        external_root: required_text(row, 5, &mut bytes, 24_576)?,
        external_identity: required_text(row, 6, &mut bytes, 24_576)?,
        assets: row.get(7).map_err(|e| e.to_string())?,
        files: row.get(8).map_err(|e| e.to_string())?,
    };
    let path = normal_path(&result.path)?;
    if path.components().count() != 2
        || result.path.split('/').count() != 2
        || !path.starts_with("external")
        || !Path::new(&result.external_root).is_absolute()
        || result.external_identity.is_empty()
        || !(0..=ROWS).contains(&result.assets)
        || !(0..=ROWS).contains(&result.files)
        || row.get::<_, i64>(9).map_err(|e| e.to_string())? != 1
        || row.get::<_, bool>(10).map_err(|e| e.to_string())?
        || rows.next().map_err(|e| e.to_string())?.is_some()
    {
        return Err(INELIGIBLE.into());
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
fn token() -> String {
    extern "C" {
        fn uuid_generate_random(value: *mut u8);
    }
    let mut value = [0_u8; 16];
    // SAFETY: macOS uuid.h writes exactly uuid_t's 16 bytes into the owned array.
    unsafe {
        uuid_generate_random(value.as_mut_ptr());
    }
    let hex: String = value.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

pub(crate) fn prepare(root: &Path, entry: TrashRemovalEntry) -> Result<TrashRemovalPlan> {
    if entry.id <= 0
        || entry.package_id <= 0
        || entry.name.is_empty()
        || entry.name.len() > 4096
        || root.as_os_str().as_encoded_bytes().len() > 4096
    {
        return Err("Ungültige Papierkorb-Anfrage".into());
    }
    #[cfg(target_os = "macos")]
    {
        let started = Instant::now();
        let reader = CatalogReader::open(root)?;
        let cancelled = AtomicBool::new(false);
        let _budget = QueryBudget::install(&reader.library.connection, &cancelled, started, WORK);
        let record = record(&reader.library.connection, &entry)?;
        reader.validate()?;
        Ok(TrashRemovalPlan {
            reply: PreparedTrashRemoval {
                token: token(),
                package_name: record.name,
                source_path: record.source,
                external_root: record.external_root,
                deleted_at: entry.deleted_at,
                asset_count: record.assets,
                file_count: record.files,
            },
            entry,
            relative_path: record.path,
            external_identity: record.external_identity,
            root_identity: Stamp::read(&reader.directory.metadata().map_err(|e| e.to_string())?)
                .identity(),
            catalog_identity: Stamp::read(
                &reader
                    .catalog_file()
                    .metadata()
                    .map_err(|e| e.to_string())?,
            )
            .identity(),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, entry);
        Err("Katalog-Entfernung ist derzeit nur für macOS qualifiziert".into())
    }
}

#[cfg(target_os = "macos")]
fn identities(root: &Path, reader: &CatalogReader, plan: &TrashRemovalPlan) -> Result<()> {
    let directory = anchored::root(root)?;
    if Stamp::read(&directory.metadata().map_err(|e| e.to_string())?).identity()
        != plan.root_identity
        || Stamp::read(&reader.directory.metadata().map_err(|e| e.to_string())?).identity()
            != plan.root_identity
        || anchored::child_stamp(&directory, OsStr::new("catalog.sqlite3"))?.identity()
            != plan.catalog_identity
        || Stamp::read(
            &reader
                .catalog_file()
                .metadata()
                .map_err(|e| e.to_string())?,
        )
        .identity()
            != plan.catalog_identity
    {
        return Err("Bibliothek oder Katalog wurde ersetzt; bitte erneut vorbereiten".into());
    }
    Ok(())
}

pub(crate) fn remove(root: &Path, plan: TrashRemovalPlan) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let started = Instant::now();
        let reader = CatalogReader::open_reserved(root).map_err(|e| match e {
            super::storage::ReservedOpenError::Busy(e)
            | super::storage::ReservedOpenError::Unavailable(e) => e,
        })?;
        identities(root, &reader, &plan)?;
        // The ordinary writer must close before the reader's same-inode raw descriptors and lease.
        let mut writer = Library::connect(root, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        writer
            .connection
            .busy_timeout(Duration::from_millis(100))
            .map_err(|e| e.to_string())?;
        reader.validate()?;
        identities(root, &reader, &plan)?;
        let cancelled = AtomicBool::new(false);
        let transaction = writer
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let budget = QueryBudget::install(&transaction, &cancelled, started, WORK);
        let current = record(&transaction, &plan.entry)?;
        if current.name != plan.reply.package_name
            || current.source != plan.reply.source_path
            || current.path != plan.relative_path
            || current.external_root != plan.reply.external_root
            || current.external_identity != plan.external_identity
            || current.assets != plan.reply.asset_count
            || current.files != plan.reply.file_count
        {
            return Err(
                "Paket wurde nach der Vorbereitung verändert; bitte erneut vorbereiten".into(),
            );
        }
        let triggers: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='trigger') OR EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE type='trigger')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if triggers {
            return Err(
                "Katalog mit zusätzlichen SQLite-Triggern kann nicht entfernt werden".into(),
            );
        }
        for sql in [
            "DELETE FROM external_files WHERE file_id IN (SELECT f.id FROM asset_files f JOIN assets a ON a.id=f.asset_id WHERE a.package_id=?1)",
            "DELETE FROM asset_files WHERE asset_id IN (SELECT id FROM assets WHERE package_id=?1)",
            "DELETE FROM assets WHERE package_id=?1",
            "DELETE FROM external_sources WHERE package_id=?1",
            "DELETE FROM packages WHERE id=?1",
        ] { transaction.execute(sql, [plan.entry.package_id]).map_err(|e| format!("Katalog-Entfernung zurückgerollt: {e}"))?; }
        identities(root, &reader, &plan)?;
        drop(budget);
        transaction
            .commit()
            .map_err(|e| format!("Katalog-Entfernung zurückgerollt: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, plan);
        Err("Katalog-Entfernung ist derzeit nur für macOS qualifiziert".into())
    }
}
