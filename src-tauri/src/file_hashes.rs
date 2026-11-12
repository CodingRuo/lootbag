//! Content hashes for finding the same file shipped in several packages.
use super::{
    id_list, location_row, modified_identity,
    zip_sources::{archive_identity, is_zip, Cancellable, OpenArchives},
    Library, ReferencedLocation, Result,
};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateCopy {
    pub file_id: i64,
    pub asset_id: i64,
    pub package_name: String,
    pub path: String,
}

/// Joined after `file_hashes h` of a file `f`: `o` are the active files in other assets with equal content.
const COPIES: &str =
    "JOIN file_hashes oh ON oh.hash = h.hash AND oh.size = h.size AND oh.file_id <> h.file_id
    JOIN active_asset_files o ON o.id = oh.file_id AND o.asset_id <> f.asset_id";

/// SQL condition: the file `f` has an identical copy in another active asset.
pub(super) fn has_copy() -> String {
    format!("EXISTS (SELECT 1 FROM file_hashes h {COPIES} WHERE h.file_id = f.id)")
}

fn hash(file: &mut dyn Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        match file.read(&mut buffer)? {
            0 => break,
            read => hasher.update(&buffer[..read]),
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// What a hash was taken from: the archive and CRC of a ZIP entry, else the file's modification time and size.
fn zip_stamp(zip: &ReferencedLocation) -> String {
    format!("{}|{}", zip.root_identity, zip.identity)
}

fn file_stamp(metadata: &fs::Metadata) -> Option<String> {
    Some(format!(
        "{}|{}",
        modified_identity(metadata).ok()?,
        metadata.len()
    ))
}

/// Scopes for `drop_stale_hashes`, conditions on its hash row `h`. Every duplicate group member:
/// a hashed file whose size and hash an active file shares.
const GROUPED: &str =
    "EXISTS (SELECT 1 FROM file_hashes oh JOIN active_asset_files o ON o.id = oh.file_id
    WHERE oh.hash = h.hash AND oh.size = h.size AND oh.file_id <> h.file_id)";
/// The hashed files of asset ?1 and every file sharing their size and hash.
const ASSET_GROUP: &str =
    "h.file_id IN (SELECT oh.file_id FROM asset_files af JOIN file_hashes ah ON ah.file_id = af.id
    JOIN file_hashes oh ON oh.hash = ah.hash AND oh.size = ah.size WHERE af.asset_id = ?1)";

impl Library {
    /// Forgets hashes in `scope` (SQL on `h`, `f`; ?1 is `asset`) whose file or archive changed
    /// since hashing, so duplicates leave them; offline files keep theirs. One stat per file.
    /// Returns how many were dropped.
    fn drop_stale_hashes(
        &self,
        scope: &str,
        asset: Option<i64>,
        cancel: &AtomicBool,
    ) -> Result<usize> {
        let failed = |e: rusqlite::Error| format!("Duplikatsuche fehlgeschlagen: {e}");
        let hashed = self
            .connection
            .prepare(&format!("SELECT h.file_id, h.stamp, f.relative_path, s.root, s.identity, p.relative_path, e.identity, e.modified, f.size_bytes
                FROM file_hashes h JOIN asset_files f ON f.id = h.file_id JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id
                LEFT JOIN external_sources s ON s.package_id = p.id LEFT JOIN external_files e ON e.file_id = f.id
                WHERE {scope}"))
            .map_err(failed)?
            .query_map(rusqlite::params_from_iter(asset), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?, location_row(row, 3)?))
            })
            .map_err(failed)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(failed)?;
        let mut stale = Vec::new();
        for (id, stamp, relative, external) in hashed {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            // `None` keeps the hash (offline, retried later); a source that is there but no longer the
            // catalogued one, or whose metadata cannot be read, yields a stamp that cannot match.
            let current = match external {
                Some(zip) if is_zip(&zip.root_identity) => {
                    match archive_identity(Path::new(&zip.root)) {
                        Ok(identity) => Some(format!("{identity}|{}", zip.identity)),
                        Err(_) if fs::symlink_metadata(&zip.root).is_ok() => Some(String::new()),
                        Err(_) => None,
                    }
                }
                Some(external) => {
                    let present = Path::new(&relative)
                        .strip_prefix(&external.prefix)
                        .is_ok_and(|inside| Path::new(&external.root).join(inside).exists());
                    match self.locate(&relative, Some(external)) {
                        Ok(path) => fs::metadata(path).ok().and_then(|m| file_stamp(&m)),
                        Err(_) if present => Some(String::new()),
                        Err(_) => None,
                    }
                }
                None => self
                    .locate(&relative, None)
                    .ok()
                    .and_then(|path| fs::metadata(path).ok())
                    .and_then(|metadata| file_stamp(&metadata)),
            };
            if current.is_some() && current != stamp {
                stale.push(id);
            }
        }
        if !stale.is_empty() {
            self.connection
                .execute(
                    "DELETE FROM file_hashes WHERE file_id IN (SELECT value FROM json_each(?1))",
                    [id_list(&stale)],
                )
                .map_err(failed)?;
        }
        Ok(stale.len())
    }

    /// Drops hashes of duplicate group members changed since hashing, so counts, the filter and
    /// copy lists only use hashes that still match; returns how many were dropped.
    pub fn verify_duplicates(&self) -> Result<usize> {
        self.drop_stale_hashes(GROUPED, None, &AtomicBool::new(false))
    }

    /// `verify_duplicates` for the files of one asset and their copies.
    pub fn verify_asset_duplicates(&self, asset_id: i64) -> Result<usize> {
        self.drop_stale_hashes(ASSET_GROUP, Some(asset_id), &AtomicBool::new(false))
    }

    /// Hashes up to `batch` files after the `after` cursor that share their size with another active
    /// file and have no current hash; returns (hashed, still pending after the cursor, dropped). Like
    /// `inspect_pending_models`, unavailable files get no row and are retried on a later run.
    /// A run's first batch (`after` 0) first drops hashes of any hashed file changed since.
    pub fn hash_pending_files(
        &self,
        after: &mut i64,
        batch: usize,
        cancel: &AtomicBool,
    ) -> Result<(usize, usize, usize)> {
        let failed = |e: rusqlite::Error| format!("Duplikatsuche fehlgeschlagen: {e}");
        let invalidated = if *after == 0 {
            self.drop_stale_hashes("1", None, cancel)?
        } else {
            0
        };
        // Empty files and unique sizes cannot be meaningful duplicates.
        let pending = "FROM active_asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id
            LEFT JOIN external_sources s ON s.package_id = p.id LEFT JOIN external_files e ON e.file_id = f.id
            LEFT JOIN file_hashes h ON h.file_id = f.id
            WHERE f.id > ?1 AND f.size_bytes > 0 AND (h.file_id IS NULL OR h.size IS NOT f.size_bytes)
            AND f.size_bytes IN (SELECT size_bytes FROM active_asset_files GROUP BY size_bytes HAVING COUNT(*) > 1)";
        let files = self
            .connection
            .prepare(&format!("SELECT f.id, f.size_bytes, f.relative_path, s.root, s.identity, p.relative_path, e.identity, e.modified, f.size_bytes {pending} ORDER BY f.id LIMIT ?2"))
            .map_err(failed)?
            .query_map(params![*after, batch as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, location_row(row, 3)?))
            })
            .map_err(failed)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(failed)?;
        let mut results = Vec::new();
        let mut archives = OpenArchives::new();
        for (id, size, relative, external) in files {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let hashed = match external {
                // ZIP entries are streamed; an unavailable archive is retried like an offline folder.
                Some(zip) if is_zip(&zip.root_identity) => self
                    .read_zip(&mut archives, &relative, &zip, cancel, hash)
                    .ok()
                    .map(|result| (zip_stamp(&zip), result)),
                external => self
                    .locate(&relative, external)
                    .ok()
                    .and_then(|path| fs::File::open(path).ok())
                    .and_then(|file| {
                        let stamp = file_stamp(&file.metadata().ok()?)?;
                        Some((stamp, hash(&mut Cancellable(cancel, file))))
                    }),
            };
            // A read stopped by `cancel` is neither a result nor past the cursor.
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            *after = id;
            if let Some((stamp, result)) = hashed {
                results.push((id, size, stamp, result.map_err(|e| e.to_string())));
            }
        }
        let transaction = self.connection.unchecked_transaction().map_err(failed)?;
        for (id, size, stamp, result) in &results {
            transaction
                .execute(
                    "INSERT OR REPLACE INTO file_hashes(file_id, size, hash, error, stamp)
                    SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM asset_files WHERE id = ?1)",
                    params![id, size, result.as_ref().ok(), result.as_ref().err(), stamp],
                )
                .map_err(failed)?;
        }
        transaction.commit().map_err(failed)?;
        let remaining: i64 = self
            .connection
            .query_row(&format!("SELECT COUNT(*) {pending}"), [*after], |row| {
                row.get(0)
            })
            .map_err(failed)?;
        Ok((results.len(), remaining as usize, invalidated))
    }

    /// Number of active files in other assets that share content with each asset, in one query.
    pub(super) fn duplicate_counts(&self, ids: &[i64]) -> Result<HashMap<i64, i64>> {
        let failed = |e: rusqlite::Error| format!("Duplikate nicht lesbar: {e}");
        self.connection
            .prepare(&format!("SELECT f.asset_id, COUNT(DISTINCT o.id) FROM active_asset_files f JOIN file_hashes h ON h.file_id = f.id
                {COPIES} WHERE f.asset_id IN (SELECT value FROM json_each(?1)) GROUP BY f.asset_id"))
            .map_err(failed)?
            .query_map([id_list(ids)], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(failed)?
            .collect::<std::result::Result<_, _>>()
            .map_err(failed)
    }

    /// Active files in other assets with the same content as a file of `asset_id`.
    pub fn asset_duplicates(&self, asset_id: i64) -> Result<Vec<DuplicateCopy>> {
        let failed = |e: rusqlite::Error| format!("Duplikate nicht lesbar: {e}");
        self.connection
            .prepare(&format!("SELECT DISTINCT o.id, o.asset_id, p.name, substr(o.relative_path, length(p.relative_path) + 2) AS path
                FROM active_asset_files f JOIN file_hashes h ON h.file_id = f.id {COPIES}
                JOIN assets a ON a.id = o.asset_id JOIN packages p ON p.id = a.package_id
                WHERE f.asset_id = ?1 ORDER BY p.name COLLATE NOCASE, path, o.id"))
            .map_err(failed)?
            .query_map([asset_id], |row| {
                Ok(DuplicateCopy {
                    file_id: row.get(0)?,
                    asset_id: row.get(1)?,
                    package_name: row.get(2)?,
                    path: row.get(3)?,
                })
            })
            .map_err(failed)?
            .collect::<std::result::Result<_, _>>()
            .map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use crate::library::{Asset, CatalogQuery, Library};
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
    };

    fn workspace() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "file-hashes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn pack(root: &Path, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let source = root.join(name);
        for (file, bytes) in files {
            let path = source.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        source
    }

    fn hash_all(library: &Library) -> usize {
        library
            .hash_pending_files(&mut 0, 100, &AtomicBool::new(false))
            .unwrap()
            .0
    }

    fn assets(library: &Library, duplicates: bool) -> Vec<Asset> {
        library
            .query(CatalogQuery {
                duplicates,
                ..Default::default()
            })
            .unwrap()
            .items
    }

    fn duplicates_of(library: &Library, name: &str) -> i64 {
        assets(library, false)
            .into_iter()
            .find(|asset| asset.name == name)
            .unwrap()
            .duplicates
    }

    #[test]
    fn identical_files_in_two_packages_form_a_group_and_trash_removes_it() {
        let root = workspace();
        let mut library = Library::create(&root.join("library")).unwrap();
        let first = library
            .import_folder(&pack(
                &root,
                "Pack A",
                &[
                    ("Rock.png", b"AAAAAAAAAA"),
                    ("Tree.obj", b"BBBBBBBBBB"),
                    ("Unique.wav", b"unique"),
                ],
            ))
            .unwrap();
        library
            .import_folder(&pack(&root, "Pack B", &[("Sub/Stone.png", b"AAAAAAAAAA")]))
            .unwrap();
        assert_eq!(hash_all(&library), 3, "only the three 10-byte files");
        let unique_hashed: bool = library
            .connection
            .query_row("SELECT EXISTS(SELECT 1 FROM file_hashes h JOIN asset_files f ON f.id = h.file_id WHERE f.original_filename = 'Unique.wav')", [], |row| row.get(0))
            .unwrap();
        assert!(!unique_hashed, "a unique size cannot be a duplicate");
        assert_eq!(hash_all(&library), 0, "nothing left to hash");

        assert_eq!(duplicates_of(&library, "Rock"), 1);
        assert_eq!(duplicates_of(&library, "Stone"), 1);
        assert_eq!(
            duplicates_of(&library, "Tree"),
            0,
            "same size, other content"
        );
        assert_eq!(duplicates_of(&library, "Unique"), 0);
        let filtered = assets(&library, true);
        let names = filtered.iter().map(|a| a.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, ["Rock", "Stone"]);
        assert!(filtered.iter().all(|asset| asset.duplicates > 0));
        assert_eq!(library.catalog_filters(None).unwrap().duplicates, 2);
        assert_eq!(
            library.catalog_filters(Some(first.id)).unwrap().duplicates,
            1
        );

        let rock = &filtered[0];
        let stone = &filtered[1];
        let copies = library.asset_duplicates(rock.id).unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].asset_id, stone.id);
        assert_eq!(copies[0].file_id, stone.files[0].id);
        assert_eq!(copies[0].package_name, "Pack B");
        assert_eq!(copies[0].path, "Sub/Stone.png");

        library.trash_file(stone.files[0].id).unwrap();
        assert_eq!(duplicates_of(&library, "Rock"), 0);
        assert!(assets(&library, true).is_empty());
        assert_eq!(library.catalog_filters(None).unwrap().duplicates, 0);
        assert!(library.asset_duplicates(rock.id).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_changed_in_place_is_hashed_again_and_leaves_its_duplicate_group() {
        let root = workspace();
        let mut library = Library::create(&root.join("library")).unwrap();
        library
            .import_folder(&pack(&root, "Pack A", &[("Rock.png", b"AAAAAAAAAA")]))
            .unwrap();
        library
            .import_folder(&pack(&root, "Pack B", &[("Stone.png", b"AAAAAAAAAA")]))
            .unwrap();
        assert_eq!(hash_all(&library), 2);
        assert_eq!(duplicates_of(&library, "Rock"), 1);
        let stone = assets(&library, false)
            .into_iter()
            .find(|asset| asset.name == "Stone")
            .unwrap();
        let path = library.resolve_preview(stone.files[0].id).unwrap();
        fs::write(&path, b"BBBBBBBBBB").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();

        assert_eq!(hash_all(&library), 1, "only the changed file");
        assert_eq!(duplicates_of(&library, "Rock"), 0);
        assert_eq!(duplicates_of(&library, "Stone"), 0);
        assert_eq!(hash_all(&library), 0, "its new stamp is current");
        let _ = fs::remove_dir_all(root);
    }

    /// Same length, other bytes and time: an edit without a hashing run.
    fn rewrite(path: &Path) {
        fs::write(path, b"BBBBBBBBBB").unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();
    }

    fn file_of(library: &Library, name: &str) -> PathBuf {
        let asset = assets(library, false)
            .into_iter()
            .find(|asset| asset.name == name)
            .unwrap();
        library.resolve_preview(asset.files[0].id).unwrap()
    }

    #[test]
    fn verify_drops_changed_members_of_every_group_and_totals_follow() {
        let root = workspace();
        let mut library = Library::create(&root.join("library")).unwrap();
        let rock = b"AAAAAAAAAA".as_slice();
        let tree = b"CCCCCCCCCC".as_slice();
        library
            .import_folder(&pack(
                &root,
                "Pack A",
                &[("Rock.png", rock), ("Tree.png", tree)],
            ))
            .unwrap();
        library
            .import_folder(&pack(
                &root,
                "Pack B",
                &[("Stone.png", rock), ("Log.png", tree)],
            ))
            .unwrap();
        assert_eq!(hash_all(&library), 4);
        assert_eq!(library.catalog_filters(None).unwrap().duplicates, 4);
        rewrite(&file_of(&library, "Stone"));

        assert_eq!(library.verify_duplicates().unwrap(), 1);
        assert_eq!(library.catalog_filters(None).unwrap().duplicates, 2);
        let names = assets(&library, true)
            .into_iter()
            .map(|asset| asset.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["Log", "Tree"]);
        assert_eq!(library.verify_duplicates().unwrap(), 0, "nothing left");
        assert_eq!(hash_all(&library), 1, "the dropped file is hashed again");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verify_covers_material_members_that_are_not_the_representative() {
        let root = workspace();
        let mut library = Library::create(&root.join("library")).unwrap();
        library
            .import_folder(&pack(
                &root,
                "Bricks",
                &[
                    ("Brick_basecolor.png", b"unique base color"),
                    ("Brick_normal.png", b"AAAAAAAAAA"),
                ],
            ))
            .unwrap();
        library
            .import_folder(&pack(&root, "Other", &[("Wall_normal.png", b"AAAAAAAAAA")]))
            .unwrap();
        assert_eq!(hash_all(&library), 2);
        let materials = |library: &Library| {
            library
                .query(CatalogQuery {
                    workflow: Some(crate::library::ImageWorkflow::Material),
                    duplicates: true,
                    ..Default::default()
                })
                .unwrap()
                .items
                .into_iter()
                .map(|asset| asset.image_group.unwrap().name)
                .collect::<Vec<_>>()
        };
        assert_eq!(materials(&library), ["Brick", "Wall"]);
        rewrite(&file_of(&library, "Brick_normal"));

        assert_eq!(library.verify_duplicates().unwrap(), 1);
        assert!(materials(&library).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verifying_one_asset_checks_its_files_and_their_copies_only() {
        let root = workspace();
        let mut library = Library::create(&root.join("library")).unwrap();
        let rock = b"AAAAAAAAAA".as_slice();
        let tree = b"CCCCCCCCCC".as_slice();
        library
            .import_folder(&pack(
                &root,
                "Pack A",
                &[("Rock.png", rock), ("Tree.png", tree)],
            ))
            .unwrap();
        library
            .import_folder(&pack(
                &root,
                "Pack B",
                &[("Stone.png", rock), ("Log.png", tree)],
            ))
            .unwrap();
        assert_eq!(hash_all(&library), 4);
        rewrite(&file_of(&library, "Stone"));
        rewrite(&file_of(&library, "Log"));
        let id = |name: &str| {
            assets(&library, false)
                .into_iter()
                .find(|asset| asset.name == name)
                .unwrap()
                .id
        };

        assert_eq!(library.verify_asset_duplicates(id("Rock")).unwrap(), 1);
        assert!(library.asset_duplicates(id("Rock")).unwrap().is_empty());
        assert_eq!(duplicates_of(&library, "Tree"), 1, "other group unchecked");
        assert_eq!(library.verify_asset_duplicates(id("Log")).unwrap(), 1);
        assert_eq!(duplicates_of(&library, "Tree"), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_zip_whose_metadata_is_unreadable_drops_its_hashes_but_a_missing_one_keeps_them() {
        let root = workspace();
        let mut library = Library::create(&root.join("library")).unwrap();
        let archive = root.join("pack.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for name in ["A/step.wav", "B/stomp.wav"] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, b"same bytes").unwrap();
        }
        zip.finish().unwrap();
        library.reference_zip(&archive).unwrap();
        assert_eq!(hash_all(&library), 2);
        assert_eq!(duplicates_of(&library, "step"), 1);

        let moved = root.join("moved.zip");
        fs::rename(&archive, &moved).unwrap();
        assert_eq!(library.verify_duplicates().unwrap(), 0, "offline: kept");
        assert_eq!(duplicates_of(&library, "step"), 1);
        fs::create_dir(&archive).unwrap();
        assert_eq!(
            library.verify_duplicates().unwrap(),
            2,
            "present but unreadable"
        );
        assert_eq!(duplicates_of(&library, "step"), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_changed_external_file_leaves_its_duplicate_group() {
        let root = workspace();
        let mut library = Library::create(&root.join("library")).unwrap();
        library
            .import_folder(&pack(&root, "Managed", &[("Hit.wav", b"same sound")]))
            .unwrap();
        let external = pack(&root, "External", &[("Hit Copy.wav", b"same sound")]);
        library.reference_folder(&external).unwrap();
        assert_eq!(hash_all(&library), 2);
        assert_eq!(duplicates_of(&library, "Hit"), 1);
        let path = external.join("Hit Copy.wav");
        fs::write(&path, b"other bits").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();

        assert_eq!(
            library
                .hash_pending_files(&mut 0, 100, &AtomicBool::new(false))
                .unwrap(),
            (0, 0, 1),
            "changed file is unavailable, not rehashed; its dropped hash is reported"
        );
        assert_eq!(duplicates_of(&library, "Hit"), 0, "stale hash dropped");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_offline_file_is_skipped_and_hashed_once_its_source_returns() {
        let root = workspace();
        let mut library = Library::create(&root.join("library")).unwrap();
        library
            .import_folder(&pack(&root, "Managed", &[("Hit.wav", b"same sound")]))
            .unwrap();
        let external = pack(&root, "External", &[("Hit Copy.wav", b"same sound")]);
        library.reference_folder(&external).unwrap();
        let moved = root.join("Moved");
        fs::rename(&external, &moved).unwrap();
        let (hashed, remaining, _) = library
            .hash_pending_files(&mut 0, 100, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            (hashed, remaining),
            (1, 0),
            "offline file skipped, cursor past it"
        );
        assert_eq!(duplicates_of(&library, "Hit"), 0);
        fs::rename(&moved, &external).unwrap();
        assert_eq!(hash_all(&library), 1, "retried on the next run");
        assert_eq!(duplicates_of(&library, "Hit"), 1);
        assert_eq!(duplicates_of(&library, "Hit Copy"), 1);
        let _ = fs::remove_dir_all(root);
    }
}
