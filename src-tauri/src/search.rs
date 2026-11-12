//! Auto-tags derived from file and folder names, the FTS5 search index and saved searches.
use super::{AssetKind, ImageWorkflow, Library, Result};
use rusqlite::{params, Connection};
use std::sync::atomic::{AtomicBool, Ordering};

/// Tokens that say nothing about an asset (incl. format folders like `FBX/`). Shorter tokens
/// (sm, sk, fx, a, v1 …) are dropped anyway.
const AUTO_TAG_STOPWORDS: &[&str] = &[
    "tex", "mod", "the", "and", "for", "with", "lod", "copy", "final", "new", "old", "fbx", "obj",
    "png", "tga", "wav",
];
const AUTO_TAG_LIMIT: usize = 8;

/// Splits `text` on everything but letters and digits, and between letters and digits.
fn segments(text: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut previous: Option<char> = None;
    for c in text.chars() {
        let boundary = !c.is_alphanumeric()
            || previous.is_some_and(|p| p.is_alphanumeric() && p.is_numeric() != c.is_numeric());
        if boundary && !parts.last().is_some_and(String::is_empty) {
            parts.push(String::new());
        }
        if c.is_alphanumeric() {
            parts.last_mut().unwrap().push(c);
        }
        previous = Some(c);
    }
    parts.retain(|part| !part.is_empty());
    parts
}

/// camelCase parts of a letter segment: `BckStrafeB` → Bck, Strafe, B; `HTTPServer` → HTTP, Server.
fn camel_parts(segment: &str) -> Vec<String> {
    let chars = segment.chars().collect::<Vec<_>>();
    let mut parts = vec![String::new()];
    for (i, &c) in chars.iter().enumerate() {
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if i > 0
            && c.is_uppercase()
            && (chars[i - 1].is_lowercase() || (chars[i - 1].is_uppercase() && next_lower))
        {
            parts.push(String::new());
        }
        parts.last_mut().unwrap().push(c);
    }
    parts
}

/// Up to `AUTO_TAG_LIMIT` lowercase tags: whole name segments of the filename stems, then the
/// folder segments deepest first, then camelCase parts. Numbers, tokens under three characters
/// and stopwords are dropped.
pub(crate) fn auto_tags(stems: &[&str], folder: &str) -> Vec<String> {
    let mut wholes = stems
        .iter()
        .flat_map(|stem| segments(stem))
        .collect::<Vec<_>>();
    wholes.extend(folder.rsplit(['/', '\\']).flat_map(segments));
    let parts = wholes
        .iter()
        .flat_map(|s| camel_parts(s))
        .collect::<Vec<_>>();
    let mut tags = Vec::new();
    for token in wholes.iter().chain(&parts).map(|s| s.to_lowercase()) {
        if tags.len() == AUTO_TAG_LIMIT {
            break;
        }
        if token.chars().count() >= 3
            && !token.chars().all(char::is_numeric)
            && !AUTO_TAG_STOPWORDS.contains(&token.as_str())
            && !tags.contains(&token)
        {
            tags.push(token);
        }
    }
    tags
}

/// FTS5 MATCH expression with every typed word as a quoted prefix, or `None` when a word has
/// characters the index tokenizer splits on (then the substring search handles it exactly).
pub(crate) fn fts_match(text: &str) -> Option<String> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.is_empty()
        || !words
            .iter()
            .all(|word| word.chars().all(char::is_alphanumeric))
    {
        return None;
    }
    Some(
        words
            .iter()
            .map(|word| format!("\"{word}\"*"))
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

/// Creates auto-tags, saved searches and the search index with triggers that keep it in sync
/// with every write to names, files, tags and packages; a new index is filled once.
pub(crate) fn create_schema(connection: &Connection) -> Result<()> {
    let failed = |e: rusqlite::Error| format!("Suchindex kann nicht angelegt werden: {e}");
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS auto_tags (asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE, tag TEXT NOT NULL, PRIMARY KEY(asset_id, tag));
            CREATE TABLE IF NOT EXISTS saved_searches (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE, query_json TEXT NOT NULL, created INTEGER NOT NULL DEFAULT (unixepoch()));",
        )
        .map_err(failed)?;
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'file_search')",
            [],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if exists {
        return Ok(());
    }
    // One row per file (rowid = file id), so adding a file never re-tokenizes its siblings.
    // Trashed rows stay indexed; the catalog query joins active files, so trash needs no update.
    let mut sql = format!(
        "BEGIN;
        CREATE VIRTUAL TABLE file_search USING fts5(body);
        CREATE VIEW file_search_text AS SELECT f.id, a.name || ' ' || p.name || ' ' || {} || ' ' || f.original_filename
            || ' ' || coalesce((SELECT group_concat(tag, ' ') FROM asset_tags WHERE asset_id = a.id), '')
            || ' ' || coalesce((SELECT group_concat(tag, ' ') FROM auto_tags WHERE asset_id = a.id), '')
            || ' ' || coalesce((SELECT group_concat(tag, ' ') FROM package_tags WHERE package_id = p.id), '') AS body
            FROM asset_files f JOIN assets a ON a.id = f.asset_id JOIN packages p ON p.id = a.package_id;
        INSERT INTO file_search(rowid, body) SELECT id, body FROM file_search_text;",
        super::FILE_FOLDER
    );
    for (table, files) in [
        ("asset_files", "SELECT {}.id"),
        ("assets", "SELECT id FROM asset_files WHERE asset_id = {}.id"),
        ("asset_tags", "SELECT id FROM asset_files WHERE asset_id = {}.asset_id"),
        ("auto_tags", "SELECT id FROM asset_files WHERE asset_id = {}.asset_id"),
        ("packages", "SELECT f.id FROM asset_files f JOIN assets a ON a.id = f.asset_id WHERE a.package_id = {}.id"),
        ("package_tags", "SELECT f.id FROM asset_files f JOIN assets a ON a.id = f.asset_id WHERE a.package_id = {}.package_id"),
    ] {
        for (event, rows) in [("INSERT", &["new"][..]), ("DELETE", &["old"]), ("UPDATE", &["old", "new"])] {
            let ids = rows.iter().map(|row| files.replace("{}", row)).collect::<Vec<_>>().join(" UNION ");
            sql.push_str(&format!(
                "CREATE TRIGGER file_search_{table}_{event} AFTER {event} ON {table} BEGIN
                    DELETE FROM file_search WHERE rowid IN ({ids});
                    INSERT INTO file_search(rowid, body) SELECT id, body FROM file_search_text WHERE id IN ({ids});
                END;",
                event = event.to_lowercase()
            ));
        }
    }
    sql.push_str("COMMIT;");
    connection.execute_batch(&sql).map_err(|e| {
        let _ = connection.execute_batch("ROLLBACK");
        failed(e)
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearch {
    pub id: i64,
    pub name: String,
    /// Opaque to the backend: the frontend's search text and filters.
    pub query_json: String,
}

/// The search state a saved search may hold: the frontend's fields with their types, nothing else.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)] // deserialized only to validate
struct SavedQuery {
    text: Option<String>,
    kind: Option<AssetKind>,
    workflow: Option<ImageWorkflow>,
    package_id: Option<i64>,
    folder_id: Option<i64>,
    original_folder: Option<String>,
    format: Option<String>,
    tag: Option<String>,
    duplicates: Option<bool>,
    audio_filters: Option<SavedAudioFilters>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)]
struct SavedAudioFilters {
    min_duration_seconds: String,
    max_duration_seconds: String,
    channels: String,
    analysis: String,
}

fn valid_saved_query(json: &str) -> bool {
    serde_json::from_str::<SavedQuery>(json).is_ok_and(|query| {
        query.audio_filters.is_none_or(|audio| {
            matches!(audio.channels.as_str(), "any" | "mono" | "stereo" | "multi")
                && matches!(
                    audio.analysis.as_str(),
                    "any" | "notAnalysed" | "ready" | "error"
                )
        })
    })
}

impl Library {
    /// Derives auto-tags for up to `batch` assets after the `after` cursor that have none yet
    /// (no file reads); returns (processed, still pending after the cursor). User tags are untouched.
    pub fn tag_pending_assets(
        &self,
        after: &mut i64,
        batch: usize,
        cancel: &AtomicBool,
    ) -> Result<(usize, usize)> {
        let failed = |e: rusqlite::Error| format!("Auto-Tags können nicht erzeugt werden: {e}");
        let pending = "FROM assets a WHERE a.id > ?1 AND NOT EXISTS (SELECT 1 FROM auto_tags t WHERE t.asset_id = a.id)";
        let assets = self
            .connection
            .prepare(&format!("SELECT a.id, a.original_folder,
                (SELECT json_group_array(DISTINCT f.original_filename) FROM asset_files f WHERE f.asset_id = a.id)
                {pending} ORDER BY a.id LIMIT ?2"))
            .map_err(failed)?
            .query_map(params![*after, batch as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .map_err(failed)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(failed)?;
        let transaction = self.connection.unchecked_transaction().map_err(failed)?;
        let mut processed = 0;
        for (id, folder, files) in assets {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            *after = id;
            processed += 1;
            let files: Vec<String> = serde_json::from_str(&files).unwrap_or_default();
            let stems = files
                .iter()
                .map(|name| {
                    std::path::Path::new(name)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(name)
                })
                .collect::<Vec<_>>();
            for tag in auto_tags(&stems, &folder) {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO auto_tags(asset_id, tag) VALUES (?1, ?2)",
                        params![id, tag],
                    )
                    .map_err(failed)?;
            }
        }
        transaction.commit().map_err(failed)?;
        let remaining: i64 = self
            .connection
            .query_row(&format!("SELECT COUNT(*) {pending}"), [*after], |row| {
                row.get(0)
            })
            .map_err(failed)?;
        Ok((processed, remaining as usize))
    }

    /// Forgets all auto-tags so the next backfill derives them again with the current rules.
    pub fn reset_auto_tags(&self) -> Result<()> {
        self.connection
            .execute("DELETE FROM auto_tags", [])
            .map(|_| ())
            .map_err(|e| format!("Auto-Tags können nicht zurückgesetzt werden: {e}"))
    }

    pub fn saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let failed = |e: rusqlite::Error| format!("Gespeicherte Suchen nicht lesbar: {e}");
        self.connection
            .prepare("SELECT id, name, query_json FROM saved_searches ORDER BY name COLLATE NOCASE")
            .map_err(failed)?
            .query_map([], |row| {
                Ok(SavedSearch {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    query_json: row.get(2)?,
                })
            })
            .map_err(failed)?
            .collect::<std::result::Result<_, _>>()
            .map_err(failed)
    }

    /// Saves under `name`, replacing the query of an existing search with the same name.
    pub fn save_search(&self, name: &str, query_json: &str) -> Result<SavedSearch> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err("Der Name muss 1 bis 100 Zeichen enthalten".into());
        }
        if query_json.len() > 10_000 || !valid_saved_query(query_json) {
            return Err("Ungültige Suche".into());
        }
        self.connection
            .query_row(
                "INSERT INTO saved_searches(name, query_json) VALUES (?1, ?2)
                ON CONFLICT(name) DO UPDATE SET query_json = excluded.query_json RETURNING id, name, query_json",
                params![name, query_json],
                |row| Ok(SavedSearch { id: row.get(0)?, name: row.get(1)?, query_json: row.get(2)? }),
            )
            .map_err(|e| format!("Suche kann nicht gespeichert werden: {e}"))
    }

    pub fn delete_saved_search(&self, id: i64) -> Result<()> {
        self.connection
            .execute("DELETE FROM saved_searches WHERE id = ?1", [id])
            .map(|_| ())
            .map_err(|e| format!("Suche kann nicht gelöscht werden: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_tags_split_names_and_folders_and_drop_noise() {
        assert_eq!(
            auto_tags(&["SM_Prop_Barrel_Nuke_01"], ""),
            ["prop", "barrel", "nuke"]
        );
        assert_eq!(
            auto_tags(
                &["A_Crouch_RootMotion_Femn"],
                "Animations/Polygon/Feminine/Locomotion/Crouch"
            ),
            [
                "crouch",
                "rootmotion",
                "femn",
                "locomotion",
                "feminine",
                "polygon",
                "animations",
                "root"
            ]
        );
        // camelCase parts, digits split off and dropped, version/LOD noise and short tokens removed.
        assert_eq!(
            auto_tags(&["WoodenCrate_LOD0_v01"], ""),
            ["woodencrate", "wooden", "crate"]
        );
        assert_eq!(
            auto_tags(&["HTTPServer"], ""),
            ["httpserver", "http", "server"]
        );
        assert_eq!(
            auto_tags(&["Zombie_Attack_Bite_001"], "SFX/Monsters/WAV"),
            ["zombie", "attack", "bite", "monsters", "sfx"]
        );
        assert_eq!(
            auto_tags(&["one_two_three_four_five_six_seven_eight_nine_ten"], "").len(),
            AUTO_TAG_LIMIT
        );
        assert!(auto_tags(&["01_002"], "").is_empty());
    }

    #[test]
    fn fts_match_quotes_prefix_words_and_rejects_separators() {
        assert_eq!(
            fts_match("barr  nuk").as_deref(),
            Some("\"barr\"* AND \"nuk\"*")
        );
        assert_eq!(fts_match("NOT or").as_deref(), Some("\"NOT\"* AND \"or\"*"));
        assert_eq!(fts_match("SM_Prop"), None);
        assert_eq!(fts_match("\"x"), None);
        assert_eq!(fts_match("   "), None);
    }
}
