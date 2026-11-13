//! Rescanning a package: reconciles a fresh discovery with its catalog and keeps annotations.
use super::{
    assets, discover_external, file_hashes::file_stamp, file_identity, id_list, insert_variant,
    managed_dir, path_text, zip_sources, AssetKind, ImportedFile, Library, Result,
};
use rusqlite::params;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RescanSummary {
    pub added: usize,
    pub changed: usize,
    /// Active files no longer found; their records and annotations stay until trashed.
    pub missing: usize,
    /// Material sets and sprite sequences ("folder/name") that became ambiguous.
    pub ambiguous: Vec<String>,
}

/// A discovered file; `stamp` identifies its content independently of hashes, so an in-place
/// change of equal size is detected: managed files through their `scan_stamps` row (see
/// `scan_stamp`), external files through their recorded modification time, ZIP entries their CRC.
pub(super) struct ScannedFile {
    pub(super) file: ImportedFile,
    pub(super) stamp: Option<String>,
}

struct CatalogFile {
    id: i64,
    asset_id: i64,
    size: i64,
    trashed: bool,
    stamp: Option<String>,
}

/// Automatic variant grouping as in `insert_variant`: folder, file stem and kind.
type GroupKey = (String, String, AssetKind);

fn group_key(relative: &Path, format: &str) -> Option<GroupKey> {
    Some((
        path_text(relative.parent().unwrap_or(Path::new(""))).ok()?,
        relative.file_stem()?.to_str()?.to_owned(),
        AssetKind::for_format(format)?,
    ))
}

impl Library {
    /// Reads a package's files again and applies the result in one transaction; a failed scan
    /// changes nothing. Managed packages read their library storage (never the original import
    /// location); external folders read their registered root after verifying its identity and
    /// accept the updated files there, so `resolve_external` keeps validating them. ZIPs read the
    /// archive at their registered location and accept it as a whole (see `rescan_zip`).
    pub fn rescan_package(&mut self, package_id: i64) -> Result<RescanSummary> {
        let (prefix, external): (String, Option<(String, String)>) = self
            .connection
            .query_row(
                "SELECT p.relative_path, s.root, s.identity FROM active_packages p
                LEFT JOIN external_sources s ON s.package_id = p.id
                WHERE p.id = ?1",
                [package_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, Option<String>>(1)?
                            .zip(row.get::<_, Option<String>>(2)?),
                    ))
                },
            )
            .map_err(|_| "Nur aktive Pakete können neu eingelesen werden")?;
        if let Some((root, identity)) = &external {
            if super::unity_packages::is_unity(identity) {
                return Err("Unity-Pakete können noch nicht neu eingelesen werden. Bitte das geänderte Paket neu einbinden. Es wurde nichts geändert.".into());
            }
            if zip_sources::is_zip(identity) {
                return self.rescan_zip(package_id, &prefix, root);
            }
        }
        let mut scanned = Vec::new();
        let mut origins = Vec::new();
        if let Some((root, identity)) = &external {
            let root = Path::new(root);
            if !fs::symlink_metadata(root).is_ok_and(|m| {
                m.is_dir()
                    && !m.file_type().is_symlink()
                    && file_identity(root, &m).ok().as_deref() == Some(identity)
            }) {
                return Err(
                    "Externe Quelle ist offline oder wurde ersetzt. Es wurde nichts geändert."
                        .into(),
                );
            }
            let mut found = Vec::new();
            discover_external(root, Path::new(""), &mut found)
                .map_err(|e| format!("{e}. Es wurde nichts geändert."))?;
            found.sort_by(|a, b| a.relative.cmp(&b.relative));
            for file in found {
                scanned.push(ScannedFile {
                    stamp: Some(format!("{}|{}", file.modified, file.size)),
                    file: ImportedFile {
                        relative: file.relative,
                        size: file.size,
                        format: file.format,
                    },
                });
                origins.push((file.identity, file.modified));
            }
        } else {
            let Some(storage) = managed_dir(&self.root, &prefix) else {
                return Err(
                    "Paketordner in der Bibliothek fehlt. Es wurde nichts geändert.".into(),
                );
            };
            scan_managed(&storage, Path::new(""), &mut scanned)?;
            scanned.sort_by(|a, b| a.file.relative.cmp(&b.file.relative));
        }
        let failed = |e: rusqlite::Error| format!("Paket kann nicht neu eingelesen werden: {e}");
        // Unchecked so the derived image groups below read the uncommitted state.
        let transaction = self.connection.unchecked_transaction().map_err(failed)?;
        let (summary, _) = self.apply_scan(package_id, &prefix, &scanned, origins)?;
        transaction.commit().map_err(failed)?;
        Ok(summary)
    }

    /// Reconciles inside the caller's transaction and records the origins of external files;
    /// returns the summary and the file ID of every scanned file in order.
    fn apply_scan(
        &self,
        package_id: i64,
        prefix: &str,
        scanned: &[ScannedFile],
        origins: Vec<(String, String)>,
    ) -> Result<(RescanSummary, Vec<i64>)> {
        let failed = |e: rusqlite::Error| format!("Paket kann nicht neu eingelesen werden: {e}");
        let before = self.ambiguous_image_groups(package_id)?;
        let (mut summary, ids) = self.reconcile(package_id, prefix, scanned)?;
        // Trashed files keep their old origin, so a restored changed file stays unresolvable until rescanned.
        for (id, (identity, modified)) in ids.iter().zip(origins) {
            self.connection
                .execute(
                    "INSERT OR REPLACE INTO external_files(file_id, identity, modified)
                    SELECT ?1, ?2, ?3 WHERE NOT EXISTS (SELECT 1 FROM trash WHERE file_id = ?1)",
                    params![id, identity, modified],
                )
                .map_err(failed)?;
        }
        summary.ambiguous = self
            .ambiguous_image_groups(package_id)?
            .difference(&before)
            .cloned()
            .collect();
        Ok((summary, ids))
    }

    /// Rescans a ZIP in place: the archive now at its registered location is validated whole with
    /// the ZIP rules, entries are reconciled by path and CRC, and the new archive identity, entry
    /// CRCs and hash stamps are stored together. An invalid archive, or one that changes before the
    /// commit, leaves the package as it was. The package's extraction cache is dropped afterwards.
    fn rescan_zip(&mut self, package_id: i64, prefix: &str, root: &str) -> Result<RescanSummary> {
        let (identity, files) = zip_sources::read_directory(Path::new(root))
            .map_err(|e| format!("{e}. Es wurde nichts geändert."))?;
        let (scanned, origins): (Vec<_>, Vec<_>) = files
            .into_iter()
            .map(|file| {
                (
                    ScannedFile {
                        stamp: Some(file.identity.clone()),
                        file: ImportedFile {
                            relative: file.relative,
                            size: file.size,
                            format: file.format,
                        },
                    },
                    (file.identity, file.modified),
                )
            })
            .unzip();
        let failed = |e: rusqlite::Error| format!("Paket kann nicht neu eingelesen werden: {e}");
        let transaction = self.connection.unchecked_transaction().map_err(failed)?;
        let (summary, ids) = self.apply_scan(package_id, prefix, &scanned, origins)?;
        // Hashes of unchanged entries stay valid for the new archive (changed ones were dropped).
        self.connection
            .execute(
                "UPDATE file_hashes SET stamp = ?1 || '|' || (SELECT e.identity FROM external_files e WHERE e.file_id = file_hashes.file_id)
                WHERE file_id IN (SELECT value FROM json_each(?2))
                AND file_id NOT IN (SELECT file_id FROM trash)",
                params![identity, id_list(&ids)],
            )
            .map_err(failed)?;
        self.connection
            .execute(
                "UPDATE external_sources SET identity = ?1 WHERE package_id = ?2",
                params![identity, package_id],
            )
            .map_err(failed)?;
        if zip_sources::archive_identity(Path::new(root)).ok() != Some(identity) {
            return Err(
                "ZIP-Datei wurde während des Einlesens verändert. Es wurde nichts geändert.".into(),
            );
        }
        transaction.commit().map_err(failed)?;
        self.remove_zip_cache(prefix);
        Ok(summary)
    }

    /// Matches `scanned` (package-relative paths) against the package's catalog inside the
    /// caller's transaction. Same path keeps the file and asset IDs; trashed files are left as
    /// they are; new files join an existing asset only when exactly one asset is still an
    /// untouched automatic group for their folder, stem and kind. Returns the file ID of every
    /// scanned file in order, so sources with extra per-file records can write them.
    pub(super) fn reconcile(
        &self,
        package_id: i64,
        prefix: &str,
        scanned: &[ScannedFile],
    ) -> Result<(RescanSummary, Vec<i64>)> {
        let failed = |e: rusqlite::Error| format!("Paket kann nicht neu eingelesen werden: {e}");
        let rows = self
            .connection
            .prepare(
                "SELECT f.id, f.asset_id, f.relative_path, f.format, f.size_bytes,
                EXISTS(SELECT 1 FROM trash t WHERE t.file_id = f.id),
                CASE e.modified WHEN 'zip' THEN e.identity
                ELSE COALESCE(s.stamp, e.modified || '|' || f.size_bytes) END
                FROM asset_files f JOIN assets a ON a.id = f.asset_id
                LEFT JOIN scan_stamps s ON s.file_id = f.id
                LEFT JOIN external_files e ON e.file_id = f.id WHERE a.package_id = ?1",
            )
            .map_err(failed)?
            .query_map([package_id], |row| {
                Ok((
                    CatalogFile {
                        id: row.get(0)?,
                        asset_id: row.get(1)?,
                        size: row.get(4)?,
                        trashed: row.get(5)?,
                        stamp: row.get(6)?,
                    },
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(failed)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(failed)?;
        let mut catalog = HashMap::new();
        let mut asset_keys = HashMap::<i64, HashSet<Option<GroupKey>>>::new();
        let mut active_assets = HashSet::new();
        for (file, path, format) in rows {
            let relative = Path::new(&path)
                .strip_prefix(prefix)
                .map_err(|_| "Ungültiger Dateipfad im Katalog")?
                .to_owned();
            asset_keys
                .entry(file.asset_id)
                .or_default()
                .insert(group_key(&relative, &format));
            if !file.trashed {
                active_assets.insert(file.asset_id);
            }
            catalog.insert(relative, file);
        }
        // Split, reassigned and fully trashed assets are not candidates; two candidates are ambiguous.
        let mut candidates = HashMap::<GroupKey, Vec<i64>>::new();
        for (asset_id, keys) in asset_keys {
            if let ([Some(key)], true) = (
                &keys.into_iter().collect::<Vec<_>>()[..],
                active_assets.contains(&asset_id),
            ) {
                candidates.entry(key.clone()).or_default().push(asset_id);
            }
        }
        let mut groups = candidates
            .into_iter()
            .filter_map(|(key, assets)| (assets.len() == 1).then(|| (key, assets[0])))
            .collect::<HashMap<_, _>>();

        let mut summary = RescanSummary::default();
        let mut ids = Vec::with_capacity(scanned.len());
        let (mut seen, mut changed, mut added) = (HashSet::new(), Vec::new(), Vec::new());
        // Unstamped files (catalogs before version 6) may have changed unseen: their derived data
        // is dropped like a change's, but they are not reported as changed.
        let mut unknown = Vec::new();
        for scan in scanned {
            if let Some(known) = catalog.get(&scan.file.relative) {
                seen.insert(known.id);
                ids.push(known.id);
                if known.trashed {
                    continue;
                }
                if known.size != scan.file.size
                    || (known.stamp.is_some() && known.stamp != scan.stamp)
                {
                    self.connection
                        .execute(
                            "UPDATE asset_files SET size_bytes = ?1 WHERE id = ?2",
                            params![scan.file.size, known.id],
                        )
                        .map_err(failed)?;
                    changed.push(known.id);
                } else if known.stamp.is_none() {
                    unknown.push(known.id);
                }
            } else {
                let id = insert_variant(
                    &self.connection,
                    package_id,
                    prefix,
                    &scan.file,
                    &mut groups,
                )?;
                ids.push(id);
                added.push(id);
            }
        }
        // Derived data is recomputed by the backfill (models, hashes, auto-tags) or on open (audio, card waveforms).
        for table in [
            "audio_metadata",
            "model_traits",
            "file_hashes",
            "audio_waveforms",
        ] {
            self.connection
                .execute(
                    &format!(
                        "DELETE FROM {table} WHERE file_id IN (SELECT value FROM json_each(?1))"
                    ),
                    [id_list(
                        &changed.iter().chain(&unknown).copied().collect::<Vec<_>>(),
                    )],
                )
                .map_err(failed)?;
        }
        for (scan, id) in scanned.iter().zip(&ids) {
            if let Some(stamp) = scan.stamp.as_deref() {
                self.connection
                    .execute(
                        "INSERT OR REPLACE INTO scan_stamps(file_id, stamp)
                        SELECT ?1, ?2 WHERE NOT EXISTS (SELECT 1 FROM trash WHERE file_id = ?1)",
                        params![id, stamp],
                    )
                    .map_err(failed)?;
            }
        }
        self.connection
            .execute(
                "DELETE FROM auto_tags WHERE asset_id IN (SELECT asset_id FROM asset_files WHERE id IN (SELECT value FROM json_each(?1)))",
                [id_list(&added)],
            )
            .map_err(failed)?;
        summary.added = added.len();
        summary.changed = changed.len();
        summary.missing = catalog
            .values()
            .filter(|file| !file.trashed && !seen.contains(&file.id))
            .count();
        Ok((summary, ids))
    }
}

/// SQL for the catalog's current state of file `id` (an SQL expression): its size and the
/// scan or origin stamps a rescan replaces when the file changed; NULL once the file is gone.
/// Background results read with one value are committed only while it still holds, in the
/// same statement, so a rescan in between makes them stale instead of restoring them.
pub(super) fn catalog_stamp(id: &str) -> String {
    format!(
        "(SELECT cf.size_bytes || '|' || COALESCE(cs.stamp, '') || '|' || COALESCE(ce.identity, '')
        || '|' || COALESCE(ce.modified, '') FROM asset_files cf
        LEFT JOIN scan_stamps cs ON cs.file_id = cf.id LEFT JOIN external_files ce ON ce.file_id = cf.id
        WHERE cf.id = {id})"
    )
}

/// Stamp of a managed file: modification time and size, plus its inode where the platform has
/// one, so a same-size replacement is noticed even when it keeps the modification time.
fn scan_stamp(metadata: &fs::Metadata) -> Option<String> {
    let stamp = file_stamp(metadata)?;
    #[cfg(unix)]
    let stamp = format!("{stamp}|{}", std::os::unix::fs::MetadataExt::ino(metadata));
    Some(stamp)
}

/// Records the scan stamp of a freshly imported managed file at `path`.
pub(super) fn save_scan_stamp(
    connection: &rusqlite::Connection,
    file_id: i64,
    path: &Path,
) -> Result<()> {
    let stamp = fs::symlink_metadata(path)
        .ok()
        .and_then(|m| scan_stamp(&m))
        .ok_or("Importierte Datei nicht lesbar")?;
    connection
        .execute(
            "INSERT OR REPLACE INTO scan_stamps(file_id, stamp) VALUES (?1, ?2)",
            params![file_id, stamp],
        )
        .map_err(|e| format!("Katalog kann nicht aktualisiert werden: {e}"))?;
    Ok(())
}

/// Discovery below a managed package folder with the import rules; links and special files fail.
fn scan_managed(root: &Path, relative: &Path, files: &mut Vec<ScannedFile>) -> Result<()> {
    for entry in fs::read_dir(root.join(relative))
        .map_err(|e| format!("Paketordner nicht lesbar. Es wurde nichts geändert: {e}"))?
    {
        let entry = entry
            .map_err(|e| format!("Paketordner nicht lesbar. Es wurde nichts geändert: {e}"))?;
        let path: PathBuf = relative.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path()).map_err(|e| {
            format!(
                "Datei nicht lesbar ({}). Es wurde nichts geändert: {e}",
                path.display()
            )
        })?;
        if metadata.is_dir() {
            scan_managed(root, &path, files)?;
        } else if metadata.is_file() {
            let Some(format) = assets::candidate_format(&path) else {
                continue;
            };
            files.push(ScannedFile {
                stamp: scan_stamp(&metadata),
                file: ImportedFile {
                    relative: path,
                    size: i64::try_from(metadata.len())
                        .map_err(|_| "Datei ist zu groß für den Katalog")?,
                    format: format.into(),
                },
            });
        } else {
            return Err(format!(
                "Nur reguläre Dateien werden eingelesen: {}. Es wurde nichts geändert.",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{AssetPackage, CatalogQuery, PackageInfo};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn setup(files: &[(&str, &[u8])]) -> (Workspace, Library, AssetPackage, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let workspace = Workspace(std::env::temp_dir().join(format!(
            "rescan-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let source = workspace.0.join("Pack");
        for (file, bytes) in files {
            write(&source.join(file), bytes);
        }
        let mut library = Library::create(&workspace.0.join("Library")).unwrap();
        let package = library.import_folder(&source).unwrap();
        let storage = library.root().join(&package.relative_path);
        (workspace, library, package, storage)
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn file_id(library: &Library, name: &str) -> i64 {
        library
            .connection
            .query_row(
                "SELECT id FROM asset_files WHERE original_filename = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn asset_of(library: &Library, file: i64) -> i64 {
        library
            .connection
            .query_row(
                "SELECT asset_id FROM asset_files WHERE id = ?1",
                [file],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn count(library: &Library, sql: &str) -> i64 {
        library
            .connection
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn add_change_and_remove_keep_ids_annotations_and_memberships() {
        let (_workspace, mut library, package, storage) = setup(&[
            ("Fx/Hit.wav", b"hit"),
            ("Fx/Gone.wav", b"gone"),
            ("Fx/Model.glb", b"glb"),
        ]);
        let hit = file_id(&library, "Hit.wav");
        let model = file_id(&library, "Model.glb");
        let asset = asset_of(&library, hit);
        library
            .annotate_asset(asset, "Punch", &["impact".into()])
            .unwrap();
        library
            .annotate_package(package.id, &["fx".into()])
            .unwrap();
        let info = PackageInfo {
            author: Some("Someone".into()),
            ..Default::default()
        };
        library.set_package_info(package.id, &info).unwrap();
        let folder = library.create_folder("Game", None).unwrap();
        library.add_to_folder(folder.id, asset).unwrap();
        library
            .set_folder_member(folder.id, asset, Some(hit), Some("Hit"))
            .unwrap();
        library
            .connection
            .execute_batch(&format!(
                "INSERT INTO audio_metadata(file_id, duration_seconds) VALUES ({hit}, 1.0);
            INSERT INTO model_traits(file_id, has_mesh, has_animation) VALUES ({model}, 1, 0);
            INSERT INTO auto_tags(asset_id, tag) VALUES ({asset}, 'stale');"
            ))
            .unwrap();

        write(&storage.join("Fx/Hit.wav"), b"hit, but longer");
        fs::remove_file(storage.join("Fx/Gone.wav")).unwrap();
        write(&storage.join("Fx/Hit.ogg"), b"ogg");
        write(&storage.join("Fx/New.wav"), b"new");
        write(&storage.join("Fx/notes.txt"), b"not an asset");
        let summary = library.rescan_package(package.id).unwrap();
        assert_eq!(
            summary,
            RescanSummary {
                added: 2,
                changed: 1,
                missing: 1,
                ambiguous: vec![]
            }
        );

        assert_eq!(
            file_id(&library, "Hit.wav"),
            hit,
            "same path keeps the file"
        );
        assert_eq!(
            asset_of(&library, file_id(&library, "Hit.ogg")),
            asset,
            "joins its untouched group"
        );
        assert_ne!(asset_of(&library, file_id(&library, "New.wav")), asset);
        let size: i64 = library
            .connection
            .query_row(
                "SELECT size_bytes FROM asset_files WHERE id = ?1",
                [hit],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(size, 15);
        assert_eq!(
            count(
                &library,
                &format!("SELECT COUNT(*) FROM audio_metadata WHERE file_id = {hit}")
            ),
            0,
            "changed file is analysed again"
        );
        assert_eq!(
            count(
                &library,
                &format!("SELECT COUNT(*) FROM model_traits WHERE file_id = {model}")
            ),
            1,
            "unchanged file keeps its traits"
        );
        assert_eq!(
            count(
                &library,
                &format!("SELECT COUNT(*) FROM auto_tags WHERE asset_id = {asset}")
            ),
            0,
            "auto-tags are derived again"
        );
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM asset_files WHERE original_filename = 'Gone.wav'"
            ),
            1,
            "missing is not deleted"
        );
        assert_eq!(
            count(&library, "SELECT COUNT(*) FROM trash"),
            0,
            "missing is not trashed"
        );

        let found = library
            .query(CatalogQuery {
                text: "punch".into(),
                ..Default::default()
            })
            .unwrap()
            .items;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, asset);
        assert_eq!(found[0].tags, vec!["impact".to_string()]);
        assert_eq!(found[0].files.len(), 2, "search index sees the new variant");
        assert_eq!(found[0].folder_members[0].file_id, Some(hit));
        assert_eq!(found[0].folder_members[0].role.as_deref(), Some("Hit"));
        let packages = library.packages().unwrap();
        assert_eq!(packages[0].tags, vec!["fx".to_string()]);
        assert_eq!(packages[0].info.author.as_deref(), Some("Someone"));

        let again = library.rescan_package(package.id).unwrap();
        assert_eq!((again.added, again.changed, again.missing), (0, 0, 1));
    }

    fn derived(library: &Library, file: i64) -> i64 {
        count(
            library,
            &format!(
                "SELECT (SELECT COUNT(*) FROM audio_metadata WHERE file_id = {file})
                + (SELECT COUNT(*) FROM model_traits WHERE file_id = {file})"
            ),
        )
    }

    #[test]
    fn same_size_managed_edits_are_changes_without_a_hash() {
        let (_workspace, mut library, package, storage) =
            setup(&[("Hit.wav", b"hit"), ("Model.glb", b"glb")]);
        let (hit, model) = (file_id(&library, "Hit.wav"), file_id(&library, "Model.glb"));
        library
            .connection
            .execute_batch(&format!(
                "INSERT INTO audio_metadata(file_id, duration_seconds) VALUES ({hit}, 1.0);
                INSERT INTO model_traits(file_id, has_mesh, has_animation) VALUES ({model}, 1, 0);"
            ))
            .unwrap();
        assert_eq!(count(&library, "SELECT COUNT(*) FROM file_hashes"), 0);
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&storage.join("Hit.wav"), b"HIT");
        write(&storage.join("Model.glb.new"), b"GLB");
        fs::rename(storage.join("Model.glb.new"), storage.join("Model.glb")).unwrap();
        let summary = library.rescan_package(package.id).unwrap();
        assert_eq!((summary.added, summary.changed), (0, 2));
        assert_eq!(derived(&library, hit) + derived(&library, model), 0);
        let again = library.rescan_package(package.id).unwrap();
        assert_eq!(again.changed, 0);
    }

    #[test]
    fn upgraded_catalogs_baseline_unstamped_files_conservatively() {
        let (workspace, library, package, storage) = setup(&[("Hit.wav", b"hit")]);
        let hit = file_id(&library, "Hit.wav");
        library
            .connection
            .execute_batch(&format!(
                "DROP TABLE scan_stamps; PRAGMA user_version = 5;
                INSERT INTO audio_metadata(file_id, duration_seconds) VALUES ({hit}, 1.0);"
            ))
            .unwrap();
        drop(library);
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&storage.join("Hit.wav"), b"HIT");
        let mut library = Library::open(&workspace.0.join("Library")).unwrap();
        let version: i64 = library
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 6);
        let summary = library.rescan_package(package.id).unwrap();
        assert_eq!(summary.changed, 0, "no stamp: a change cannot be claimed");
        assert_eq!(derived(&library, hit), 0, "but derived data is recomputed");
        assert_eq!(
            count(&library, "SELECT COUNT(*) FROM scan_stamps"),
            1,
            "baselined"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&storage.join("Hit.wav"), b"hiT");
        assert_eq!(library.rescan_package(package.id).unwrap().changed, 1);
    }

    #[test]
    fn background_results_read_before_a_rescan_are_not_committed() {
        let (_workspace, mut library, package, storage) =
            setup(&[("Model.glb", b"glb"), ("Hit.wav", b"hit")]);
        let (model, hit) = (file_id(&library, "Model.glb"), file_id(&library, "Hit.wav"));
        let stamp = |library: &Library, id: i64| -> String {
            library
                .connection
                .query_row(&format!("SELECT {}", catalog_stamp("?1")), [id], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        let (old_model, old_hit) = (stamp(&library, model), stamp(&library, hit));
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&storage.join("Model.glb"), b"GLB");
        write(&storage.join("Hit.wav"), b"HIT");
        assert_eq!(library.rescan_package(package.id).unwrap().changed, 2);

        let go = std::sync::atomic::AtomicBool::new(false);
        let traits =
            |stamp: String| vec![(model, stamp, Ok(crate::library::ModelTraits::default()))];
        let hashes = |stamp: String| vec![(hit, 3, "old".to_owned(), stamp, Ok("h".to_owned()))];
        library.save_model_traits(&traits(old_model), &go).unwrap();
        library.save_hashes(&hashes(old_hit), &go).unwrap();
        assert_eq!(derived(&library, model), 0, "stale traits are rejected");
        assert_eq!(count(&library, "SELECT COUNT(*) FROM file_hashes"), 0);

        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let (new_model, new_hit) = (stamp(&library, model), stamp(&library, hit));
        library
            .save_model_traits(&traits(new_model.clone()), &cancelled)
            .unwrap();
        library
            .save_hashes(&hashes(new_hit.clone()), &cancelled)
            .unwrap();
        assert_eq!(derived(&library, model), 0, "nothing after cancellation");
        assert_eq!(count(&library, "SELECT COUNT(*) FROM file_hashes"), 0);

        library.save_model_traits(&traits(new_model), &go).unwrap();
        library.save_hashes(&hashes(new_hit), &go).unwrap();
        assert_eq!(derived(&library, model), 1, "current results are kept");
        assert_eq!(count(&library, "SELECT COUNT(*) FROM file_hashes"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn linked_managed_package_folders_are_neither_rescanned_nor_read_for_licences() {
        let (workspace, mut library, package, storage) = setup(&[("Hit.wav", b"hit")]);
        let outside = workspace.0.join("Outside");
        write(&outside.join("Hit.wav"), b"hit");
        write(&outside.join("New.wav"), b"new");
        write(&outside.join("LICENSE.txt"), b"Released under CC0 1.0.");
        let real = workspace.0.join("Real");
        fs::rename(&storage, &real).unwrap();
        std::os::unix::fs::symlink(&outside, &storage).unwrap();
        let details = library.get_package_info(package.id).unwrap();
        assert!(details.license_files.is_empty(), "the link is not followed");
        assert!(details.license_suggestions.is_empty());
        assert!(library.rescan_package(package.id).is_err());

        // A linked ancestor (`packages`) is refused the same way.
        fs::remove_file(&storage).unwrap();
        fs::rename(&real, &storage).unwrap();
        write(&storage.join("LICENSE.txt"), b"Released under CC0 1.0.");
        assert_eq!(
            library.get_package_info(package.id).unwrap().license_files,
            vec!["LICENSE.txt".to_string()]
        );
        let packages = library.root().join("packages");
        let moved = workspace.0.join("Packages");
        fs::rename(&packages, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &packages).unwrap();
        assert!(library
            .get_package_info(package.id)
            .unwrap()
            .license_files
            .is_empty());
        assert!(library.rescan_package(package.id).is_err());
        assert_eq!(count(&library, "SELECT COUNT(*) FROM asset_files"), 1);
    }

    #[test]
    fn trash_and_manual_splits_survive_and_renames_are_missing_plus_new() {
        let (_workspace, mut library, package, storage) = setup(&[
            ("Step.wav", b"wav"),
            ("Step.ogg", b"ogg"),
            ("Trashed.wav", b"t"),
            ("TrashedGone.wav", b"tg"),
            ("Old.wav", b"old"),
            ("run_01.png", b"1"),
            ("run_02.png", b"2"),
        ]);
        let split = library
            .split_variant(file_id(&library, "Step.ogg"))
            .unwrap();
        let trashed = file_id(&library, "Trashed.wav");
        library.trash_file(trashed).unwrap();
        library
            .trash_file(file_id(&library, "TrashedGone.wav"))
            .unwrap();

        write(&storage.join("Trashed.wav"), b"changed while trashed");
        fs::remove_file(storage.join("TrashedGone.wav")).unwrap();
        fs::rename(storage.join("Old.wav"), storage.join("Renamed.wav")).unwrap();
        write(&storage.join("Step.mp3"), b"mp3");
        write(&storage.join("run_1.png"), b"dup frame");
        let summary = library.rescan_package(package.id).unwrap();
        assert_eq!(
            summary,
            RescanSummary {
                added: 3,
                changed: 0,
                missing: 1,
                ambiguous: vec!["run_".into()]
            }
        );

        assert_eq!(file_id(&library, "Trashed.wav"), trashed);
        assert_eq!(
            count(&library, "SELECT COUNT(*) FROM trash"),
            2,
            "trash is untouched"
        );
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM active_asset_files WHERE original_filename LIKE 'Trashed%'"
            ),
            0,
            "never resurrected"
        );
        assert_eq!(
            asset_of(&library, file_id(&library, "Step.ogg")),
            split,
            "split stays"
        );
        let mp3 = asset_of(&library, file_id(&library, "Step.mp3"));
        assert!(
            mp3 != split && mp3 != asset_of(&library, file_id(&library, "Step.wav")),
            "ambiguous group gets its own asset"
        );
    }

    #[test]
    fn failed_scans_and_other_packages_leave_the_catalog_unchanged() {
        let (workspace, mut library, package, storage) = setup(&[("A.wav", b"a")]);
        write(&storage.join("B.wav"), b"b");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(storage.join("A.wav"), storage.join("Link.wav")).unwrap();
            assert!(library.rescan_package(package.id).is_err());
            assert_eq!(
                count(&library, "SELECT COUNT(*) FROM asset_files"),
                1,
                "nothing applied"
            );
            fs::remove_file(storage.join("Link.wav")).unwrap();
        }

        let source = workspace.0.join("External");
        write(&source.join("C.wav"), b"c");
        let external = library.reference_folder(&source).unwrap();
        let zip = workspace.0.join("Pack.zip");
        write_zip(&zip, &[("D.wav", b"d")]);
        let zip = library.reference_zip(&zip).unwrap();
        library.trash_package(zip.id).unwrap();
        assert!(library.rescan_package(zip.id).is_err());
        library.trash_package(external.id).unwrap();
        assert!(library.rescan_package(external.id).is_err());
        library.trash_package(package.id).unwrap();
        assert!(library.rescan_package(package.id).is_err());
        assert_eq!(count(&library, "SELECT COUNT(*) FROM asset_files"), 3);
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn files_below(path: &Path) -> usize {
        fs::read_dir(path).map_or(0, |entries| {
            entries
                .flatten()
                .map(|entry| match entry.file_type() {
                    Ok(kind) if kind.is_dir() => files_below(&entry.path()),
                    _ => 1,
                })
                .sum()
        })
    }

    #[test]
    fn a_rewritten_zip_is_rescanned_in_place_and_drops_its_cache() {
        let (workspace, mut library, _, _) = setup(&[("Other.png", b"x")]);
        let archive = workspace.0.join("Fx.zip");
        write_zip(
            &archive,
            &[
                ("Fx/Hit.wav", b"hit"),
                ("Fx/Same.wav", b"same"),
                ("Fx/Gone.wav", b"gone"),
            ],
        );
        let package = library.reference_zip(&archive).unwrap();
        let (hit, same) = (file_id(&library, "Hit.wav"), file_id(&library, "Same.wav"));
        let asset = asset_of(&library, hit);
        library
            .annotate_asset(asset, "Punch", &["impact".into()])
            .unwrap();
        library
            .connection
            .execute_batch(&format!(
                "INSERT INTO audio_metadata(file_id, duration_seconds) VALUES ({hit}, 1.0);
                INSERT INTO file_hashes(file_id, size, hash, stamp) VALUES ({same}, 4, 'h', 'old');"
            ))
            .unwrap();
        library.resolve_preview(hit).unwrap();
        library.resolve_preview(same).unwrap();
        let cache = library.root().join(".cache").join(&package.relative_path);
        assert_eq!(files_below(&cache), 2);

        // Equal size: only the CRC reveals the change.
        write_zip(
            &archive,
            &[
                ("Fx/Hit.wav", b"HIT"),
                ("Fx/Same.wav", b"same"),
                ("Fx/Hit.ogg", b"ogg"),
                ("Fx/readme.txt", b"not an asset"),
            ],
        );
        let summary = library.rescan_package(package.id).unwrap();
        assert_eq!(
            summary,
            RescanSummary {
                added: 1,
                changed: 1,
                missing: 1,
                ambiguous: vec![]
            }
        );
        assert_eq!(files_below(&cache), 0, "the old cache is dropped");
        assert_eq!(
            file_id(&library, "Hit.wav"),
            hit,
            "same path keeps the file"
        );
        assert_eq!(asset_of(&library, file_id(&library, "Hit.ogg")), asset);
        assert_eq!(
            count(
                &library,
                &format!("SELECT COUNT(*) FROM audio_metadata WHERE file_id = {hit}")
            ),
            0
        );
        let stamp: String = library
            .connection
            .query_row(
                "SELECT stamp FROM file_hashes WHERE file_id = ?1",
                [same],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stamp,
            format!(
                "{}|{}",
                zip_sources::archive_identity(&archive).unwrap(),
                origin(&library, same).0
            ),
            "an unchanged entry keeps its hash for the new archive"
        );
        let found = library
            .query(CatalogQuery {
                text: "punch".into(),
                ..Default::default()
            })
            .unwrap()
            .items;
        assert_eq!(found[0].tags, vec!["impact".to_string()]);
        let preview = library.resolve_preview(hit).unwrap();
        assert_eq!(fs::read(preview).unwrap(), b"HIT");
        library
            .resolve_preview(file_id(&library, "Hit.ogg"))
            .unwrap();
        assert!(library
            .resolve_preview(file_id(&library, "Gone.wav"))
            .is_err());
        assert!(library.packages().unwrap()[1].available);
        let again = library.rescan_package(package.id).unwrap();
        assert_eq!((again.added, again.changed, again.missing), (0, 0, 1));
    }

    #[test]
    fn an_invalid_or_missing_zip_leaves_the_package_unchanged() {
        let (workspace, mut library, _, _) = setup(&[("Other.png", b"x")]);
        let archive = workspace.0.join("Fx.zip");
        write_zip(&archive, &[("Hit.wav", b"hit")]);
        let package = library.reference_zip(&archive).unwrap();
        let hit = file_id(&library, "Hit.wav");
        let identity = |library: &Library| -> String {
            library
                .connection
                .query_row(
                    "SELECT identity FROM external_sources WHERE package_id = ?1",
                    [package.id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let before = (identity(&library), origin(&library, hit));

        write_zip(&archive, &[("../escape.wav", b"x"), ("New.wav", b"new")]);
        let error = library.rescan_package(package.id).unwrap_err();
        assert!(error.contains("Es wurde nichts geändert"), "{error}");
        fs::write(&archive, b"not a zip").unwrap();
        assert!(library.rescan_package(package.id).is_err(), "corrupt");
        fs::remove_file(&archive).unwrap();
        assert!(library.rescan_package(package.id).is_err(), "missing");
        assert_eq!(count(&library, "SELECT COUNT(*) FROM asset_files"), 2);
        assert_eq!((identity(&library), origin(&library, hit)), before);
    }

    fn origin(library: &Library, file: i64) -> (String, String) {
        library
            .connection
            .query_row(
                "SELECT identity, modified FROM external_files WHERE file_id = ?1",
                [file],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    #[test]
    fn external_folders_accept_updated_files_at_their_registered_root() {
        let (workspace, mut library, _, _) = setup(&[("Other.png", b"x")]);
        let source = workspace.0.join("External");
        write(&source.join("Fx/Hit.wav"), b"hit");
        write(&source.join("Fx/Same.wav"), b"same");
        write(&source.join("Fx/Gone.wav"), b"gone");
        let package = library.reference_folder(&source).unwrap();
        let hit = file_id(&library, "Hit.wav");
        let asset = asset_of(&library, hit);
        library
            .annotate_asset(asset, "Punch", &["impact".into()])
            .unwrap();
        library
            .connection
            .execute_batch(&format!(
                "INSERT INTO audio_metadata(file_id, duration_seconds) VALUES ({hit}, 1.0);"
            ))
            .unwrap();
        let before = origin(&library, hit);

        // Equal size: only the recorded modification time reveals the change.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&source.join("Fx/Hit.wav"), b"HIT");
        fs::remove_file(source.join("Fx/Gone.wav")).unwrap();
        write(&source.join("Fx/Hit.ogg"), b"ogg");
        let summary = library.rescan_package(package.id).unwrap();
        assert_eq!(
            summary,
            RescanSummary {
                added: 1,
                changed: 1,
                missing: 1,
                ambiguous: vec![]
            }
        );
        assert_eq!(
            file_id(&library, "Hit.wav"),
            hit,
            "same path keeps the file"
        );
        assert_eq!(asset_of(&library, file_id(&library, "Hit.ogg")), asset);
        assert_ne!(
            origin(&library, hit),
            before,
            "the updated file is accepted"
        );
        assert_eq!(
            count(
                &library,
                &format!("SELECT COUNT(*) FROM audio_metadata WHERE file_id = {hit}")
            ),
            0
        );
        assert_eq!(
            count(&library, "SELECT COUNT(*) FROM external_files"),
            4,
            "the new file has an origin; the missing one keeps its record"
        );
        let found = library
            .query(CatalogQuery {
                text: "punch".into(),
                ..Default::default()
            })
            .unwrap()
            .items;
        assert_eq!(found[0].tags, vec!["impact".to_string()]);
        for name in ["Hit.wav", "Hit.ogg", "Same.wav"] {
            library
                .resolve_preview(file_id(&library, name))
                .unwrap_or_else(|e| panic!("{name} resolves: {e}"));
        }
        assert!(library
            .resolve_preview(file_id(&library, "Gone.wav"))
            .is_err());
        let again = library.rescan_package(package.id).unwrap();
        assert_eq!((again.added, again.changed, again.missing), (0, 0, 1));
    }

    #[test]
    fn offline_or_replaced_external_roots_leave_the_catalog_unchanged() {
        let (workspace, mut library, _, _) = setup(&[("Other.png", b"x")]);
        let source = workspace.0.join("External");
        write(&source.join("A.wav"), b"a");
        let package = library.reference_folder(&source).unwrap();
        let a = file_id(&library, "A.wav");
        let before = origin(&library, a);

        fs::rename(&source, workspace.0.join("Moved")).unwrap();
        assert!(library.rescan_package(package.id).is_err(), "offline");

        // A different folder at the registered path, even with a matching file.
        write(&source.join("A.wav"), b"b");
        write(&source.join("New.wav"), b"new");
        assert!(library.rescan_package(package.id).is_err(), "replaced");
        assert_eq!(count(&library, "SELECT COUNT(*) FROM asset_files"), 2);
        assert_eq!(origin(&library, a), before);

        #[cfg(unix)]
        {
            fs::remove_dir_all(&source).unwrap();
            fs::rename(workspace.0.join("Moved"), &source).unwrap();
            write(&source.join("Sub/B.wav"), b"b");
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(source.join("Sub"), fs::Permissions::from_mode(0o000)).unwrap();
            let unreadable = library.rescan_package(package.id);
            fs::set_permissions(source.join("Sub"), fs::Permissions::from_mode(0o755)).unwrap();
            assert!(unreadable.is_err(), "incomplete traversal");
            assert_eq!(count(&library, "SELECT COUNT(*) FROM asset_files"), 2);
            assert_eq!(library.rescan_package(package.id).unwrap().added, 1);
        }
    }
}
