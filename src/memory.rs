use anyhow::Context;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rand::RngExt;

use crate::embed::{self, Embedder};
use crate::session::TranscriptTuple;

pub const TOP_K: usize = 3;
const MAX_ENTRIES: usize = 1000;
const UPSERT_JACCARD: f64 = 0.7;
const UPSERT_MIN_SHARED: usize = 2;
/// Transcript hits kept at most when merging with memory hits.
const TRANSCRIPT_CAP: usize = 3;
/// KNN candidates fetched per source before the distance cutoff and merging.
const OVERFETCH: usize = 4;
/// Transcripts are pruned once processed and older than this many days.
pub const RETENTION_DAYS: i64 = 7;
/// A memory is judged once it is unused and unjudged for this many days.
pub const JUDGE_DAYS: i64 = 30;
/// Tuples fed to one extraction request, all from the same session.
pub const EXTRACT_BATCH: usize = 10;
/// Memory entries fed to one judge request.
pub const JUDGE_BATCH: usize = 10;
/// Similar entries shown to the judge as supersession evidence.
pub const SIMILAR_LIMIT: usize = 3;

static STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "he", "in", "is", "it",
    "its", "of", "on", "that", "the", "to", "was", "were", "will", "with", "what", "when", "where",
    "who", "how", "i", "you", "your", "we", "our", "they", "their", "do", "does", "did", "not",
    "no", "yes", "me", "my", "this", "these", "those",
];

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS memory (
  rowid          INTEGER PRIMARY KEY,
  id             TEXT NOT NULL UNIQUE,
  text           TEXT NOT NULL,
  tags           TEXT NOT NULL DEFAULT '[]',
  created        TEXT NOT NULL,
  updated        TEXT NOT NULL,
  last_used      TEXT NOT NULL,
  last_judged    TEXT,
  origin         TEXT NOT NULL DEFAULT 'agent',
  source_session TEXT
);

CREATE TABLE IF NOT EXISTS transcripts (
  rowid      INTEGER PRIMARY KEY,
  session    TEXT NOT NULL,
  seq        INTEGER NOT NULL,
  user_text  TEXT NOT NULL,
  agent_text TEXT NOT NULL,
  created    TEXT NOT NULL,
  processed  INTEGER NOT NULL DEFAULT 0,
  UNIQUE(session, seq)
);
"#;

fn tokenize(text: &str) -> Vec<String> {
    static TOKEN_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = TOKEN_RE.get_or_init(|| Regex::new(r"[^a-zA-Z0-9]+").unwrap());
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();
    re.split(text)
        .map(|s| s.to_lowercase())
        .filter(|s| !s.is_empty() && !stopwords.contains(s.as_str()))
        .collect()
}

fn now_iso() -> String {
    crate::util::now_iso()
}

/// Maximum length of a memory hit fragment shown or injected.
pub const FRAGMENT_CHARS: usize = 100;

/// Reduce `text` to at most [`FRAGMENT_CHARS`] characters around the first
/// occurrence of a query term. Whitespace is collapsed; `…` marks truncation.
pub fn fragment(text: &str, query: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= FRAGMENT_CHARS {
        return flat;
    }

    // Reserve two characters for the ellipsis markers so the total stays capped.
    let budget = FRAGMENT_CHARS.saturating_sub(2);
    let (start, end) = match first_match(&flat, query) {
        Some((idx, len)) => {
            let before = budget.saturating_sub(len) / 2;
            let mut start = idx.saturating_sub(before);
            let mut end = (start + budget).min(chars.len());
            if end - start < budget {
                start = end.saturating_sub(budget);
            }
            end = (start + budget).min(chars.len());
            (start, end)
        }
        None => (0, budget),
    };

    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// The earliest `(char_index, char_len)` at which any query term appears in `text`.
fn first_match(text: &str, query: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for term in tokenize(query) {
        if let Some(byte) = find_ascii_ci(text, &term) {
            let idx = text[..byte].chars().count();
            let len = term.chars().count();
            if best.is_none_or(|(i, _)| idx < i) {
                best = Some((idx, len));
            }
        }
    }
    best
}

/// Case-insensitive ASCII substring search; returns the byte offset of the match.
fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

fn iso_days_ago(days: i64) -> String {
    use time::OffsetDateTime;
    use time::format_description::FormatItem;
    use time::macros::format_description;

    let fmt: &[FormatItem] = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    (OffsetDateTime::now_utc() - time::Duration::days(days))
        .format(fmt)
        .unwrap_or_default()
}

/// A stored memory fact.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
    pub tags: Vec<String>,
    pub created: String,
    pub updated: String,
    pub last_used: String,
    #[allow(dead_code)]
    pub last_judged: Option<String>,
    pub origin: String,
    pub source_session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    Memory,
    Transcript,
}

/// A retrieval result: a memory entry or a transcript excerpt.
#[derive(Debug, Clone)]
pub struct Hit {
    pub kind: HitKind,
    pub key: String,
    pub text: String,
    pub tags: Vec<String>,
    pub session: Option<String>,
    pub created: String,
    /// Cosine similarity to the query (`1 - distance`), 0..=1; higher is better.
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct TranscriptRow {
    pub rowid: i64,
    pub session: String,
    pub seq: i64,
    pub user_text: String,
    pub agent_text: String,
}

impl TranscriptRow {
    pub fn render(&self) -> String {
        format!("user: {}\nagent: {}", self.user_text, self.agent_text)
    }
}

/// The single SQLite connection, guarded for the few short critical sections.
struct Store {
    conn: Mutex<Connection>,
    embedder: Arc<dyn Embedder>,
}

/// A handle to the shared store carrying the provenance of whatever is writing
/// through it (interactive session, or one dreaming batch). Cheap to clone.
#[derive(Clone)]
pub struct Memory {
    store: Arc<Store>,
    origin: String,
    source_session: Option<String>,
}

impl std::fmt::Debug for Memory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Memory")
            .field("origin", &self.origin)
            .field("source_session", &self.source_session)
            .finish_non_exhaustive()
    }
}

/// The database path for a configured memory path. A legacy `*.json` path maps
/// to the sibling `*.db`.
pub fn db_path_for(path: &Path) -> PathBuf {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        path.with_extension("db")
    } else {
        path.to_path_buf()
    }
}

/// Register sqlite-vec with every connection opened afterwards. Must run before
/// the first `Connection::open`.
fn register_sqlite_vec() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        use rusqlite::auto_extension::{RawAutoExtension, register_auto_extension};
        let entry: RawAutoExtension =
            std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const () as usize);
        if let Err(e) = register_auto_extension(entry) {
            log::error!("failed to register sqlite-vec: {e}");
        }
    });
}

/// Drop the FTS5 tables and their triggers left by pre-embedding databases.
fn migrate_from_fts(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS memory_ai;
         DROP TRIGGER IF EXISTS memory_ad;
         DROP TRIGGER IF EXISTS memory_au;
         DROP TRIGGER IF EXISTS transcripts_ai;
         DROP TRIGGER IF EXISTS transcripts_ad;
         DROP TRIGGER IF EXISTS transcripts_au;
         DROP TABLE IF EXISTS memory_fts;
         DROP TABLE IF EXISTS transcripts_fts;",
    )?;
    Ok(())
}

fn get_meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| {
        r.get(0)
    })
    .optional()
    .ok()
    .flatten()
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

/// Create the vector tables at the embedder's dimension, rebuilding them when
/// the configured model or dimension changed (existing rows are re-embedded
/// lazily by [`Memory::ensure_vectors`]).
fn ensure_vec_tables(conn: &Connection, dims: usize, model_id: &str) -> anyhow::Result<()> {
    let stored_dim = get_meta(conn, "embed_dim").and_then(|v| v.parse::<usize>().ok());
    let stored_model = get_meta(conn, "embed_model");
    if stored_dim != Some(dims) || stored_model.as_deref() != Some(model_id) {
        conn.execute_batch(
            "DROP TABLE IF EXISTS memory_vec;
             DROP TABLE IF EXISTS transcript_vec;",
        )?;
        log::info!("(re)building memory vector index for model '{model_id}' ({dims}d)");
    }
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS memory_vec USING vec0(embedding float[{dims}] distance_metric=cosine);
         CREATE VIRTUAL TABLE IF NOT EXISTS transcript_vec USING vec0(embedding float[{dims}] distance_metric=cosine);"
    ))?;
    set_meta(conn, "embed_dim", &dims.to_string())?;
    set_meta(conn, "embed_model", model_id)?;
    Ok(())
}

/// The text a memory entry is embedded from (tags included so they are searchable).
fn memory_passage(text: &str, tags: &[String]) -> String {
    if tags.is_empty() {
        text.to_string()
    } else {
        format!("{text}\n{}", tags.join(", "))
    }
}

fn transcript_passage(user: &str, agent: &str) -> String {
    format!("{user}\n{agent}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyEntry {
    id: String,
    text: String,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    created: String,
    #[serde(default)]
    updated: String,
    #[serde(default)]
    source_session: Option<String>,
    #[serde(default = "default_origin")]
    origin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyFile {
    #[serde(default)]
    entries: Vec<LegacyEntry>,
}

fn default_origin() -> String {
    "agent".to_string()
}

impl Memory {
    pub fn open(path: &Path, embedder: Arc<dyn Embedder>) -> anyhow::Result<Self> {
        register_sqlite_vec();
        let db = db_path_for(path);
        if !db.exists() && path.exists() {
            let count = migrate_json(path, &db)?;
            let bak = path.with_extension("json.bak");
            let _ = std::fs::rename(path, &bak);
            log::info!(
                "migrated {count} memory entries from {} to {}",
                path.display(),
                db.display()
            );
        }
        if let Some(parent) = db.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&db)
            .with_context(|| format!("opening memory database: {}", db.display()))?;
        conn.execute_batch(SCHEMA)
            .with_context(|| format!("initializing memory database: {}", db.display()))?;
        migrate_from_fts(&conn)?;
        if embedder.dims() > 0 {
            ensure_vec_tables(&conn, embedder.dims(), &embedder.model_id())?;
        }
        Ok(Self {
            store: Arc::new(Store {
                conn: Mutex::new(conn),
                embedder,
            }),
            origin: "agent".to_string(),
            source_session: None,
        })
    }

    /// A handle sharing this store but attributing writes to `source_session`.
    pub fn fork(&self, source_session: Option<&str>, origin: &str) -> Self {
        Self {
            store: Arc::clone(&self.store),
            origin: origin.to_string(),
            source_session: source_session.map(str::to_string),
        }
    }

    /// Store a fact. Near-duplicates update the existing entry in place; returns
    /// the entry id and whether an existing entry was updated.
    pub fn add(
        &self,
        text: String,
        tags: Vec<String>,
        origin: Option<&str>,
    ) -> Result<(String, bool), String> {
        let origin = origin.unwrap_or(&self.origin).to_string();
        let tags: Vec<String> = tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        let conn = self.store.conn.lock().unwrap();
        let entries = load_memory(&conn).map_err(|e| format!("failed to read memory: {e}"))?;

        if !entries.is_empty() {
            let query = tokenize(&text);
            if !query.is_empty() {
                let mut best: Option<(usize, f64)> = None;
                for (i, entry) in entries.iter().enumerate() {
                    let score = jaccard_overlap(&query, &tokenize(&entry.text));
                    if best.is_none_or(|(_, bs)| score > bs) {
                        best = Some((i, score));
                    }
                }
                if let Some((i, score)) = best
                    && score >= UPSERT_JACCARD
                    && shared_terms(&query, &tokenize(&entries[i].text)) >= UPSERT_MIN_SHARED
                {
                    let id = entries[i].id.clone();
                    let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".into());
                    conn.execute(
                        "UPDATE memory SET text=?1, tags=?2, updated=?3, origin=?4, source_session=?5 WHERE id=?6",
                        params![text, tags_json, now_iso(), origin, self.source_session, id],
                    )
                    .map_err(|e| format!("failed to save memory: {e}"))?;
                    self.reindex_memory(&conn, &id, &text, &tags);
                    return Ok((id, true));
                }
            }
        }

        if entries.len() >= MAX_ENTRIES {
            return Err(
                "Memory is full (max 1000 entries). Delete some entries first.".to_string(),
            );
        }

        let mut rng = rand::rng();
        let id = loop {
            let key: u16 = rng.random();
            let key_str = format!("{:04x}", key);
            if !entries.iter().any(|e| e.id == key_str) {
                break key_str;
            }
        };
        let now = now_iso();
        let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".into());
        conn.execute(
            "INSERT INTO memory (id, text, tags, created, updated, last_used, last_judged, origin, source_session)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4, NULL, ?5, ?6)",
            params![id, text, tags_json, now, origin, self.source_session],
        )
        .map_err(|e| format!("failed to save memory: {e}"))?;
        self.reindex_memory(&conn, &id, &text, &tags);
        Ok((id, false))
    }

    /// Best-effort rewrite of a memory entry's vector row.
    fn reindex_memory(&self, conn: &Connection, id: &str, text: &str, tags: &[String]) {
        if self.store.embedder.dims() == 0 {
            return;
        }
        let Ok(rowid) = conn.query_row("SELECT rowid FROM memory WHERE id=?1", params![id], |r| {
            r.get::<_, i64>(0)
        }) else {
            return;
        };
        if let Some(vector) = embed_one(&*self.store.embedder, &memory_passage(text, tags))
            && let Err(e) = upsert_vector(conn, "memory_vec", rowid, &vector)
        {
            log::warn!("memory vector write failed: {e}");
        }
    }

    pub fn delete(&self, key: &str) -> Result<String, String> {
        let conn = self.store.conn.lock().unwrap();
        let rowid: Option<i64> = conn
            .query_row("SELECT rowid FROM memory WHERE id=?1", params![key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| format!("failed to delete memory: {e}"))?;
        if let Some(rowid) = rowid {
            let _ = conn.execute("DELETE FROM memory_vec WHERE rowid=?1", params![rowid]);
        }
        let changed = conn
            .execute("DELETE FROM memory WHERE id=?1", params![key])
            .map_err(|e| format!("failed to delete memory: {e}"))?;
        if changed == 0 {
            return Err(format!("No memory entry with key '{key}'."));
        }
        Ok(format!("Deleted memory entry '{key}'."))
    }

    pub fn get(&self, key: &str) -> Option<MemoryEntry> {
        let conn = self.store.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, text, tags, created, updated, last_used, last_judged, origin, source_session
             FROM memory WHERE id=?1",
            params![key],
            map_entry,
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn list(&self) -> Vec<MemoryEntry> {
        let conn = self.store.conn.lock().unwrap();
        load_memory(&conn).unwrap_or_default()
    }

    pub fn count_memory(&self) -> usize {
        let conn = self.store.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM memory", [], |r| r.get::<_, i64>(0))
            .map(|n| n as usize)
            .unwrap_or(0)
    }

    /// Open maintenance work: (unprocessed transcript tuples, entries unused and
    /// unjudged past [`JUDGE_DAYS`]).
    pub fn pending_tasks(&self) -> (usize, usize) {
        let conn = self.store.conn.lock().unwrap();
        let tuples = conn
            .query_row(
                "SELECT COUNT(*) FROM transcripts WHERE processed=0",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as usize)
            .unwrap_or(0);
        let judge = conn
            .query_row(
                "SELECT COUNT(*) FROM memory
                 WHERE COALESCE(last_judged, created) < ?1 AND last_used < ?1",
                params![iso_days_ago(JUDGE_DAYS)],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as usize)
            .unwrap_or(0);
        (tuples, judge)
    }

    pub fn count_transcripts(&self) -> usize {
        let conn = self.store.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM transcripts", [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n as usize)
        .unwrap_or(0)
    }

    pub fn summary(&self) -> String {
        let entries = self.count_memory();
        let transcripts = self.count_transcripts();
        if entries == 0 && transcripts == 0 {
            "## Memory\nMemory is enabled but empty. Relevant entries and past transcript \
             excerpts are injected per message; use memory_add to store facts (optionally with tags)."
                .to_string()
        } else {
            format!(
                "## Memory\n{entries} memory entries and {transcripts} transcript excerpts stored. \
                 Relevant items are injected per message; use memory_add to store new facts \
                 (optionally with tags). Run `ai dream` to distill transcripts into memories, prune \
                 processed transcripts, and judge stale entries."
            )
        }
    }

    /// Semantic retrieval over memory and transcripts: KNN by cosine distance
    /// against the stored vectors, filtered by the embedder's distance cutoff. Memory hits are
    /// kept ahead of transcript excerpts, which are capped.
    pub fn retrieve(&self, query: &str, top_k: usize) -> Vec<Hit> {
        let vector = match self.store.embedder.embed_query(query) {
            Ok(vector) if !vector.is_empty() => vector,
            Ok(_) => return Vec::new(),
            Err(e) => {
                log::warn!("memory retrieval unavailable: {e}");
                return Vec::new();
            }
        };
        if let Err(e) = self.ensure_vectors() {
            log::warn!("memory vector backfill failed: {e}");
        }
        let blob = embed::to_blob(&vector);
        let max_distance = self.store.embedder.max_distance();
        let k = (top_k.max(1) * OVERFETCH) as i64;
        let conn = self.store.conn.lock().unwrap();

        let mut memory_hits: Vec<(f32, Hit)> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT m.id, m.text, m.tags, m.created, v.distance
             FROM memory_vec v JOIN memory m ON m.rowid = v.rowid
             WHERE v.embedding MATCH ?1 AND k = ?2
             ORDER BY v.distance",
        ) {
            let rows = stmt
                .query_map(params![blob, k], |r| {
                    let tags: String = r.get(2)?;
                    let distance = r.get::<_, f64>(4)? as f32;
                    Ok((
                        distance,
                        Hit {
                            kind: HitKind::Memory,
                            key: r.get(0)?,
                            text: r.get(1)?,
                            tags: parse_tags(&tags),
                            session: None,
                            created: r.get(3)?,
                            score: score_for(distance),
                        },
                    ))
                })
                .map(|rows| rows.flatten().collect::<Vec<_>>())
                .unwrap_or_default();
            memory_hits = rows
                .into_iter()
                .filter(|(d, _)| *d <= max_distance)
                .collect();
        }

        let mut transcript_hits: Vec<(f32, Hit)> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT t.session, t.seq, t.user_text, t.agent_text, t.created, v.distance
             FROM transcript_vec v JOIN transcripts t ON t.rowid = v.rowid
             WHERE v.embedding MATCH ?1 AND k = ?2
             ORDER BY v.distance",
        ) {
            let rows = stmt
                .query_map(params![blob, k], |r| {
                    let session: String = r.get(0)?;
                    let seq: i64 = r.get(1)?;
                    let distance = r.get::<_, f64>(5)? as f32;
                    Ok((
                        distance,
                        Hit {
                            kind: HitKind::Transcript,
                            key: format!("{session}#{seq}"),
                            text: format!(
                                "user: {}\nagent: {}",
                                r.get::<_, String>(2)?,
                                r.get::<_, String>(3)?
                            ),
                            tags: Vec::new(),
                            session: Some(session),
                            created: r.get(4)?,
                            score: score_for(distance),
                        },
                    ))
                })
                .map(|rows| rows.flatten().collect::<Vec<_>>())
                .unwrap_or_default();
            transcript_hits = rows
                .into_iter()
                .filter(|(d, _)| *d <= max_distance)
                .collect();
        }

        memory_hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        transcript_hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut out: Vec<Hit> = Vec::new();
        for (_, hit) in memory_hits {
            if out.len() >= top_k {
                break;
            }
            out.push(hit);
        }
        for (_, hit) in transcript_hits.into_iter().take(TRANSCRIPT_CAP) {
            if out.len() >= top_k {
                break;
            }
            out.push(hit);
        }

        // Retrieval is the usage signal the judge pool depends on.
        let now = now_iso();
        for hit in &out {
            if hit.kind == HitKind::Memory {
                let _ = conn.execute(
                    "UPDATE memory SET last_used=?1 WHERE id=?2",
                    params![now, hit.key],
                );
            }
        }
        out
    }

    /// Embed any source rows that lack a vector (fresh database, model change,
    /// or a write whose embedding failed). Idempotent and cheap when complete.
    fn ensure_vectors(&self) -> anyhow::Result<()> {
        let (memories, transcripts) = {
            let conn = self.store.conn.lock().unwrap();
            let memories = conn
                .prepare(
                    "SELECT rowid, text, tags FROM memory
                     WHERE rowid NOT IN (SELECT rowid FROM memory_vec)",
                )?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        memory_passage(
                            &r.get::<_, String>(1)?,
                            &parse_tags(&r.get::<_, String>(2)?),
                        ),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let transcripts = conn
                .prepare(
                    "SELECT rowid, user_text, agent_text FROM transcripts
                     WHERE rowid NOT IN (SELECT rowid FROM transcript_vec)",
                )?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        transcript_passage(&r.get::<_, String>(1)?, &r.get::<_, String>(2)?),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (memories, transcripts)
        };
        if memories.is_empty() && transcripts.is_empty() {
            return Ok(());
        }

        let write = |table: &str, rows: Vec<(i64, String)>| -> anyhow::Result<()> {
            if rows.is_empty() {
                return Ok(());
            }
            let passages: Vec<String> = rows.iter().map(|(_, p)| p.clone()).collect();
            let vectors = self.store.embedder.embed_passages(&passages)?;
            let conn = self.store.conn.lock().unwrap();
            for ((rowid, _), vector) in rows.iter().zip(vectors.iter()) {
                upsert_vector(&conn, table, *rowid, vector)?;
            }
            Ok(())
        };
        write("memory_vec", memories)?;
        write("transcript_vec", transcripts)?;
        log::info!("backfilled memory vectors");
        Ok(())
    }

    /// Store one session's user+agent tuples. Rewrites rows whose text changed
    /// (resetting `processed`) and drops rows beyond the current log, so a
    /// cleared or compacted session is reflected too.
    pub fn index_session(
        &self,
        session: &str,
        tuples: &[TranscriptTuple],
    ) -> anyhow::Result<usize> {
        let existing: HashMap<i64, (String, String)> = {
            let conn = self.store.conn.lock().unwrap();
            conn.prepare("SELECT seq, user_text, agent_text FROM transcripts WHERE session=?1")?
                .query_map(params![session], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
                    ))
                })?
                .collect::<rusqlite::Result<HashMap<_, _>>>()?
        };

        let changed: Vec<&TranscriptTuple> = tuples
            .iter()
            .filter(|t| match existing.get(&(t.seq as i64)) {
                Some((user, agent)) => user != &t.user || agent != &t.agent,
                None => true,
            })
            .collect();
        let vectors: Vec<Vec<f32>> = if changed.is_empty() || self.store.embedder.dims() == 0 {
            Vec::new()
        } else {
            let passages: Vec<String> = changed
                .iter()
                .map(|t| transcript_passage(&t.user, &t.agent))
                .collect();
            match self.store.embedder.embed_passages(&passages) {
                Ok(vectors) => vectors,
                Err(e) => {
                    log::warn!("transcript vectors skipped: {e}");
                    Vec::new()
                }
            }
        };

        let mut conn = self.store.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = now_iso();
        let mut changed_count = 0;
        for t in tuples {
            changed_count += tx.execute(
                "INSERT INTO transcripts (session, seq, user_text, agent_text, created, processed)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)
                 ON CONFLICT(session, seq) DO UPDATE SET
                   user_text = excluded.user_text,
                   agent_text = excluded.agent_text,
                   processed = 0
                 WHERE transcripts.user_text IS NOT excluded.user_text
                    OR transcripts.agent_text IS NOT excluded.agent_text",
                params![session, t.seq as i64, t.user, t.agent, now],
            )?;
        }

        let rowids: HashMap<i64, i64> = tx
            .prepare("SELECT seq, rowid FROM transcripts WHERE session=?1")?
            .query_map(params![session], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        for (t, vector) in changed.iter().zip(vectors.iter()) {
            if let Some(rowid) = rowids.get(&(t.seq as i64)) {
                upsert_vector(&tx, "transcript_vec", *rowid, vector)?;
            }
        }

        let max_seq = tuples.last().map(|t| t.seq as i64).unwrap_or(0);
        let stale: Vec<i64> = tx
            .prepare("SELECT rowid FROM transcripts WHERE session=?1 AND seq > ?2")?
            .query_map(params![session, max_seq], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for rowid in stale {
            let _ = tx.execute("DELETE FROM transcript_vec WHERE rowid=?1", params![rowid]);
        }
        tx.execute(
            "DELETE FROM transcripts WHERE session=?1 AND seq > ?2",
            params![session, max_seq],
        )?;
        tx.commit()?;
        Ok(changed_count)
    }

    /// One-time indexing of every existing session file. Guarded by `meta`.
    pub fn backfill_sessions(&self, dir: &Path) -> anyhow::Result<usize> {
        {
            let conn = self.store.conn.lock().unwrap();
            let done: Option<String> = conn
                .query_row(
                    "SELECT value FROM meta WHERE key='transcripts_backfilled'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            if done.is_some() {
                return Ok(0);
            }
        }
        let mut total = 0;
        for name in crate::session::Session::list(dir).unwrap_or_default() {
            match crate::session::Session::load(&name, dir) {
                Ok(session) => {
                    total += self.index_session(&session.name, &session.tuples())?;
                }
                Err(e) => log::warn!("backfill: skipping session {name}: {e}"),
            }
        }
        let conn = self.store.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('transcripts_backfilled', ?1)",
            params![now_iso()],
        )?;
        Ok(total)
    }

    /// Unprocessed transcript rows, grouped into batches from one session.
    pub fn unprocessed_batches(&self, size: usize) -> Vec<Vec<TranscriptRow>> {
        let conn = self.store.conn.lock().unwrap();
        let rows = load_transcripts(&conn, false).unwrap_or_default();
        let mut batches = Vec::new();
        let mut current: Vec<TranscriptRow> = Vec::new();
        for row in rows {
            if current
                .first()
                .is_some_and(|first| first.session != row.session || current.len() >= size)
            {
                batches.push(std::mem::take(&mut current));
            }
            current.push(row);
        }
        if !current.is_empty() {
            batches.push(current);
        }
        batches
    }

    pub fn mark_processed(&self, rowids: &[i64]) -> anyhow::Result<usize> {
        if rowids.is_empty() {
            return Ok(0);
        }
        let placeholders = placeholders(rowids.len(), 1);
        let conn = self.store.conn.lock().unwrap();
        let n = conn.execute(
            &format!("UPDATE transcripts SET processed=1 WHERE rowid IN ({placeholders})"),
            params_from_iter(rowids.iter()),
        )?;
        Ok(n)
    }

    pub fn prune_transcripts(&self, days: i64) -> anyhow::Result<usize> {
        let conn = self.store.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM transcript_vec WHERE rowid IN
               (SELECT rowid FROM transcripts WHERE processed=1 AND created < ?1)",
            params![iso_days_ago(days)],
        )?;
        let n = conn.execute(
            "DELETE FROM transcripts WHERE processed=1 AND created < ?1",
            params![iso_days_ago(days)],
        )?;
        Ok(n)
    }

    /// Entries unused and unjudged for `days`, oldest use first.
    pub fn judge_candidates(&self, limit: usize, days: i64) -> Vec<MemoryEntry> {
        let conn = self.store.conn.lock().unwrap();
        let cutoff = iso_days_ago(days);
        let mut stmt = match conn.prepare(
            "SELECT id, text, tags, created, updated, last_used, last_judged, origin, source_session
             FROM memory
             WHERE COALESCE(last_judged, created) < ?1 AND last_used < ?1
             ORDER BY last_used ASC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![cutoff, limit as i64], map_entry)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Semantically similar entries updated after `entry`, as supersession evidence.
    pub fn similar_newer(&self, entry: &MemoryEntry, limit: usize) -> Vec<MemoryEntry> {
        let Ok(mut vectors) = self
            .store
            .embedder
            .embed_passages(&[memory_passage(&entry.text, &entry.tags)])
        else {
            return Vec::new();
        };
        let Some(vector) = vectors.pop() else {
            return Vec::new();
        };
        let blob = embed::to_blob(&vector);
        let conn = self.store.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT m.id, m.text, m.tags, m.created, m.updated, m.last_used, m.last_judged, m.origin, m.source_session
             FROM memory_vec v JOIN memory m ON m.rowid = v.rowid
             WHERE v.embedding MATCH ?1 AND k = ?2
             ORDER BY v.distance",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![blob, (limit.max(1) * OVERFETCH) as i64], map_entry)
            .map(|rows| {
                rows.flatten()
                    .filter(|e| e.id != entry.id && e.updated > entry.updated)
                    .take(limit)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn mark_judged(&self, ids: &[String]) -> anyhow::Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders = placeholders(ids.len(), 2);
        let conn = self.store.conn.lock().unwrap();
        let now = now_iso();
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&now];
        for id in ids {
            params_vec.push(id);
        }
        let n = conn.execute(
            &format!("UPDATE memory SET last_judged=?1 WHERE id IN ({placeholders})"),
            params_vec.as_slice(),
        )?;
        Ok(n)
    }
}

fn placeholders(n: usize, start: usize) -> String {
    (start..start + n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_tags(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

/// Embed one passage, logging (rather than failing) when the model is unavailable.
fn embed_one(embedder: &dyn Embedder, passage: &str) -> Option<Vec<f32>> {
    match embedder.embed_passages(&[passage.to_string()]) {
        Ok(mut vectors) => vectors.pop(),
        Err(e) => {
            log::warn!("memory vector skipped: {e}");
            None
        }
    }
}

/// Similarity shown to users: cosine distance inverted and clamped to 0..=1.
fn score_for(distance: f32) -> f32 {
    (1.0 - distance).clamp(0.0, 1.0)
}

/// Replace a source row's vector. vec0 has no `INSERT OR REPLACE`, so delete first.
fn upsert_vector(
    conn: &Connection,
    table: &str,
    rowid: i64,
    vector: &[f32],
) -> rusqlite::Result<()> {
    conn.execute(
        &format!("DELETE FROM {table} WHERE rowid=?1"),
        params![rowid],
    )?;
    conn.execute(
        &format!("INSERT INTO {table}(rowid, embedding) VALUES (?1, ?2)"),
        params![rowid, embed::to_blob(vector)],
    )?;
    Ok(())
}

fn map_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryEntry> {
    let tags: String = r.get(2)?;
    Ok(MemoryEntry {
        id: r.get(0)?,
        text: r.get(1)?,
        tags: parse_tags(&tags),
        created: r.get(3)?,
        updated: r.get(4)?,
        last_used: r.get(5)?,
        last_judged: r.get(6)?,
        origin: r.get(7)?,
        source_session: r.get(8)?,
    })
}

fn load_memory(conn: &Connection) -> rusqlite::Result<Vec<MemoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, text, tags, created, updated, last_used, last_judged, origin, source_session
         FROM memory ORDER BY created DESC",
    )?;
    stmt.query_map([], map_entry)
        .map(|rows| rows.flatten().collect())
}

fn load_transcripts(conn: &Connection, processed: bool) -> rusqlite::Result<Vec<TranscriptRow>> {
    let mut stmt = conn.prepare(
        "SELECT rowid, session, seq, user_text, agent_text FROM transcripts
         WHERE processed = ?1 ORDER BY session, seq",
    )?;
    stmt.query_map(params![processed as i64], |r| {
        Ok(TranscriptRow {
            rowid: r.get(0)?,
            session: r.get(1)?,
            seq: r.get(2)?,
            user_text: r.get(3)?,
            agent_text: r.get(4)?,
        })
    })
    .map(|rows| rows.flatten().collect())
}

fn jaccard_overlap(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: HashSet<&str> = a.iter().map(String::as_str).collect();
    let set_b: HashSet<&str> = b.iter().map(String::as_str).collect();
    let shared = set_a.intersection(&set_b).count();
    shared as f64 / (set_a.len() + set_b.len() - shared).max(1) as f64
}

fn shared_terms(a: &[String], b: &[String]) -> usize {
    let set_a: HashSet<&str> = a.iter().map(String::as_str).collect();
    b.iter().filter(|t| set_a.contains(t.as_str())).count()
}

/// Read a legacy JSON memory file and insert its entries into a new database.
fn migrate_json(json_path: &Path, db_path: &Path) -> anyhow::Result<usize> {
    let content = std::fs::read_to_string(json_path)
        .with_context(|| format!("reading memory file: {}", json_path.display()))?;
    let entries = parse_legacy(&content)
        .with_context(|| format!("parsing memory file: {}", json_path.display()))?;

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch(SCHEMA)?;
    let now = now_iso();
    let mut count = 0;
    for e in entries {
        let created = if e.created.is_empty() {
            now.clone()
        } else {
            e.created
        };
        let updated = if e.updated.is_empty() {
            created.clone()
        } else {
            e.updated
        };
        let tags = serde_json::to_string(&e.keywords).unwrap_or_else(|_| "[]".into());
        conn.execute(
            "INSERT OR IGNORE INTO memory (id, text, tags, created, updated, last_used, last_judged, origin, source_session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL, ?6, ?7)",
            params![e.id, e.text, tags, created, updated, e.origin, e.source_session],
        )?;
        count += 1;
    }
    Ok(count)
}

fn parse_legacy(content: &str) -> anyhow::Result<Vec<LegacyEntry>> {
    let value: serde_json::Value = serde_json::from_str(content)?;
    if value.get("entries").is_some() {
        let file: LegacyFile = serde_json::from_value(value)?;
        return Ok(file.entries);
    }
    let map: HashMap<String, String> = serde_json::from_value(value)?;
    let now = now_iso();
    Ok(map
        .into_iter()
        .map(|(id, text)| LegacyEntry {
            id,
            text,
            keywords: Vec::new(),
            created: now.clone(),
            updated: now.clone(),
            source_session: None,
            origin: default_origin(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_embedder() -> Arc<dyn Embedder> {
        Arc::new(crate::embed::HashEmbedder::new(256))
    }

    fn temp_memory(name: &str) -> (PathBuf, Memory) {
        temp_memory_with(name, test_embedder())
    }

    fn temp_memory_with(name: &str, embedder: Arc<dyn Embedder>) -> (PathBuf, Memory) {
        let dir =
            std::env::temp_dir().join(format!("ai-memory-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.db");
        (dir, Memory::open(&path, embedder).unwrap())
    }

    fn cleanup(dir: PathBuf) {
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tuple(seq: usize, user: &str, agent: &str) -> TranscriptTuple {
        TranscriptTuple {
            seq,
            user: user.to_string(),
            agent: agent.to_string(),
        }
    }

    #[test]
    fn test_empty_memory() {
        let (dir, mem) = temp_memory("empty");
        assert_eq!(mem.retrieve("anything", 5).len(), 0);
        assert!(mem.summary().contains("empty"));
        cleanup(dir);
    }

    #[test]
    fn test_add_and_retrieve_by_tags() {
        let (dir, mem) = temp_memory("retrieve");
        mem.add(
            "user prefers dark mode".to_string(),
            vec!["dark mode".to_string(), "preference".to_string()],
            None,
        )
        .unwrap();
        let hits = mem.retrieve("dark mode please", 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].text, "user prefers dark mode");
        assert_eq!(hits[0].kind, HitKind::Memory);
        cleanup(dir);
    }

    #[test]
    fn test_keyword_boost_outweighs_low_text_overlap() {
        let (dir, mem) = temp_memory("tagboost");
        mem.add(
            "user's favorite food".to_string(),
            vec!["pizza".to_string()],
            None,
        )
        .unwrap();
        mem.add(
            "some unrelated tech note".to_string(),
            vec!["rust".to_string()],
            None,
        )
        .unwrap();
        let hits = mem.retrieve("pizza", 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].text, "user's favorite food");
        cleanup(dir);
    }

    #[test]
    fn test_near_duplicate_upsert() {
        let (dir, mem) = temp_memory("upsert");
        let (id1, updated1) = mem
            .add("user prefers dark mode".to_string(), vec![], None)
            .unwrap();
        assert!(!updated1);
        let (id2, updated2) = mem
            .add("the user prefers dark mode theme".to_string(), vec![], None)
            .unwrap();
        assert_eq!(id1, id2, "near-duplicate should update the existing entry");
        assert!(updated2, "second add should report an update");
        assert_eq!(mem.count_memory(), 1);
        cleanup(dir);
    }

    #[test]
    fn test_distinct_entries_shared_tag_not_merged() {
        let (dir, mem) = temp_memory("distinct");
        let id1 = mem
            .add(
                "user prefers dark mode".to_string(),
                vec!["preference".to_string()],
                None,
            )
            .unwrap()
            .0;
        let id2 = mem
            .add(
                "user's favorite color is blue".to_string(),
                vec!["preference".to_string()],
                None,
            )
            .unwrap()
            .0;
        assert_ne!(id1, id2);
        assert_eq!(mem.count_memory(), 2);
        cleanup(dir);
    }

    #[test]
    fn test_delete_and_origin() {
        let (dir, mem) = temp_memory("delete");
        let key = mem.add("data".to_string(), vec![], Some("user")).unwrap().0;
        assert_eq!(mem.get(&key).unwrap().origin, "user");
        assert!(mem.delete(&key).is_ok());
        assert!(mem.delete(&key).is_err());
        cleanup(dir);
    }

    #[test]
    fn test_get_unknown_key_is_none() {
        let (dir, mem) = temp_memory("getmissing");
        assert!(mem.get("nope").is_none());
        cleanup(dir);
    }

    #[test]
    fn test_retrieve_refreshes_last_used() {
        let (dir, mem) = temp_memory("lastused");
        let key = mem
            .add("user lives in berlin".to_string(), vec![], None)
            .unwrap()
            .0;
        let before = mem.get(&key).unwrap().last_used;
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(!mem.retrieve("berlin", 5).is_empty());
        let after = mem.get(&key).unwrap().last_used;
        assert_ne!(before, after, "retrieval should refresh last_used");
        cleanup(dir);
    }

    #[test]
    fn test_transcripts_rank_below_memory() {
        let (dir, mem) = temp_memory("rrf");
        mem.add("berlin is the capital".to_string(), vec![], None)
            .unwrap();
        mem.index_session("s1", &[tuple(1, "tell me about berlin", "berlin is nice")])
            .unwrap();
        let hits = mem.retrieve("berlin", 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].kind,
            HitKind::Memory,
            "memory should outrank transcript"
        );
        assert_eq!(hits[1].kind, HitKind::Transcript);
        cleanup(dir);
    }

    #[test]
    fn test_index_session_idempotent() {
        let (dir, mem) = temp_memory("idem");
        let tuples = [tuple(1, "hello", "hi"), tuple(2, "bye", "ok")];
        assert_eq!(mem.index_session("s1", &tuples).unwrap(), 2);
        assert_eq!(mem.index_session("s1", &tuples).unwrap(), 0);
        assert_eq!(mem.count_transcripts(), 2);
        cleanup(dir);
    }

    #[test]
    fn test_prune_only_processed_and_old() {
        let (dir, mem) = temp_memory("prune");
        mem.index_session("s1", &[tuple(1, "old", "reply")])
            .unwrap();
        let rowid = {
            let conn = mem.store.conn.lock().unwrap();
            load_transcripts(&conn, false).unwrap()[0].rowid
        };
        // Unprocessed rows survive regardless of age.
        mem.prune_transcripts(0).unwrap();
        assert_eq!(mem.count_transcripts(), 1);
        mem.mark_processed(&[rowid]).unwrap();
        // Processed but not older than the window survives.
        mem.prune_transcripts(RETENTION_DAYS).unwrap();
        assert_eq!(mem.count_transcripts(), 1);
        // Processed and older than the window is removed.
        {
            let conn = mem.store.conn.lock().unwrap();
            conn.execute(
                "UPDATE transcripts SET created=?1 WHERE rowid=?2",
                params![iso_days_ago(RETENTION_DAYS + 1), rowid],
            )
            .unwrap();
        }
        mem.prune_transcripts(RETENTION_DAYS).unwrap();
        assert_eq!(mem.count_transcripts(), 0);
        cleanup(dir);
    }

    #[test]
    fn test_pending_tasks_counts() {
        let (dir, mem) = temp_memory("pending");
        assert_eq!(mem.pending_tasks(), (0, 0));

        mem.index_session("s1", &[tuple(1, "u1", "a1"), tuple(2, "u2", "a2")])
            .unwrap();
        let key = mem.add("stale fact".to_string(), vec![], None).unwrap().0;
        assert_eq!(mem.pending_tasks(), (2, 0));

        {
            let conn = mem.store.conn.lock().unwrap();
            conn.execute(
                "UPDATE memory SET created=?1, last_used=?1 WHERE id=?2",
                params![iso_days_ago(JUDGE_DAYS + 1), key],
            )
            .unwrap();
        }
        assert_eq!(mem.pending_tasks(), (2, 1));

        let rowid = {
            let conn = mem.store.conn.lock().unwrap();
            load_transcripts(&conn, false).unwrap()[0].rowid
        };
        mem.mark_processed(&[rowid]).unwrap();
        assert_eq!(mem.pending_tasks().0, 1);
        cleanup(dir);
    }

    #[test]
    fn test_judge_pool_windows() {
        let (dir, mem) = temp_memory("judge");
        let key = mem.add("stale fact".to_string(), vec![], None).unwrap().0;
        // Fresh entries are not candidates.
        assert!(mem.judge_candidates(10, JUDGE_DAYS).is_empty());
        // Backdate both created and last_used.
        {
            let conn = mem.store.conn.lock().unwrap();
            conn.execute(
                "UPDATE memory SET created=?1, last_used=?1 WHERE id=?2",
                params![iso_days_ago(JUDGE_DAYS + 1), key],
            )
            .unwrap();
        }
        let candidates = mem.judge_candidates(10, JUDGE_DAYS);
        assert_eq!(candidates.len(), 1);
        // Marking it judged removes it from the pool again.
        mem.mark_judged(&[candidates[0].id.clone()]).unwrap();
        assert!(mem.judge_candidates(10, JUDGE_DAYS).is_empty());
        cleanup(dir);
    }

    #[test]
    fn test_similar_newer_only_returns_newer() {
        let (dir, mem) = temp_memory("similar");
        let old = mem
            .add("user lives in berlin".to_string(), vec![], None)
            .unwrap()
            .0;
        // Backdate the first entry, then add a newer restatement.
        {
            let conn = mem.store.conn.lock().unwrap();
            conn.execute(
                "UPDATE memory SET updated='2000-01-01T00:00:00Z' WHERE id=?1",
                params![old],
            )
            .unwrap();
        }
        mem.add("user has moved to paris".to_string(), vec![], None)
            .unwrap();
        let entry = mem.get(&old).unwrap();
        let newer = mem.similar_newer(&entry, SIMILAR_LIMIT);
        assert_eq!(newer.len(), 1);
        assert!(newer[0].text.contains("paris"));
        cleanup(dir);
    }

    #[test]
    fn test_backfill_sessions_once() {
        use crate::session::Session;

        let dir = std::env::temp_dir().join(format!("ai-memory-backfill-{}", std::process::id()));
        let sessions = dir.join("sessions");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&sessions).unwrap();

        let mut s = Session::new(
            "2026-09-16_test".into(),
            "sys".into(),
            "m".into(),
            "openai".into(),
        );
        s.add_user("I use vim");
        s.add_assistant("noted");
        s.save(&sessions).unwrap();

        let mem = Memory::open(&dir.join("memory.db"), test_embedder()).unwrap();
        assert_eq!(mem.backfill_sessions(&sessions).unwrap(), 1);
        assert_eq!(mem.backfill_sessions(&sessions).unwrap(), 0);
        assert_eq!(mem.count_transcripts(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_migrate_legacy_json() {
        let dir = std::env::temp_dir().join(format!("ai-memory-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.json");
        std::fs::write(
            &path,
            r#"{"version":2,"entries":[{"id":"a1b2","text":"legacy fact","keywords":["kw"],"created":"2020-01-01T00:00:00Z","updated":"2020-01-02T00:00:00Z","source_session":"s","origin":"user"}]}"#,
        )
        .unwrap();

        let mem = Memory::open(&path, test_embedder()).unwrap();
        assert!(path.with_extension("json.bak").exists());
        assert!(!path.exists());
        let all = mem.list();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].tags, vec!["kw".to_string()]);
        assert_eq!(all[0].origin, "user");
        assert_eq!(all[0].last_used, "2020-01-02T00:00:00Z");
        assert!(mem.retrieve("legacy fact", 5).len() == 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_migrate_legacy_map() {
        let dir = std::env::temp_dir().join(format!("ai-memory-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.json");
        std::fs::write(&path, r#"{"a1b2": "legacy fact"}"#).unwrap();
        let mem = Memory::open(&path, test_embedder()).unwrap();
        assert_eq!(mem.list().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_distance_cutoff_excludes_unrelated() {
        let (dir, mem) = temp_memory_with(
            "cutoff",
            Arc::new(crate::embed::HashEmbedder::with_max_distance(256, 0.7)),
        );
        mem.add("user lives in berlin".to_string(), vec![], None)
            .unwrap();
        assert_eq!(mem.retrieve("berlin", 5).len(), 1);
        assert!(
            mem.retrieve("quantum chromodynamics lecture notes", 5)
                .is_empty(),
            "unrelated query should fall outside the distance cutoff"
        );
        cleanup(dir);
    }

    #[test]
    fn test_single_term_query_still_matches() {
        let (dir, mem) = temp_memory("singleterm");
        mem.add("user lives in berlin".to_string(), vec![], None)
            .unwrap();
        assert_eq!(mem.retrieve("berlin", 5).len(), 1);
        cleanup(dir);
    }

    #[test]
    fn test_vectors_track_memory_lifecycle() {
        let (dir, mem) = temp_memory("veclife");
        let key = mem
            .add("user lives in berlin".to_string(), vec![], None)
            .unwrap()
            .0;
        assert_eq!(vector_count(&mem, "memory_vec"), 1);
        mem.delete(&key).unwrap();
        assert_eq!(vector_count(&mem, "memory_vec"), 0);
        cleanup(dir);
    }

    #[test]
    fn test_ensure_vectors_backfills_missing() {
        let (dir, mem) = temp_memory("vecbackfill");
        mem.add("user lives in berlin".to_string(), vec![], None)
            .unwrap();
        mem.store
            .conn
            .lock()
            .unwrap()
            .execute("DELETE FROM memory_vec", [])
            .unwrap();
        assert_eq!(mem.retrieve("berlin", 5).len(), 1);
        assert_eq!(vector_count(&mem, "memory_vec"), 1);
        cleanup(dir);
    }

    #[test]
    fn test_embedding_model_change_rebuilds_vectors() {
        let dir =
            std::env::temp_dir().join(format!("ai-memory-test-vecmodel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.db");
        let mem = Memory::open(&path, Arc::new(crate::embed::HashEmbedder::new(256))).unwrap();
        mem.add("user lives in berlin".to_string(), vec![], None)
            .unwrap();
        drop(mem);

        let mem = Memory::open(&path, Arc::new(crate::embed::HashEmbedder::new(16))).unwrap();
        assert_eq!(
            get_meta(&mem.store.conn.lock().unwrap(), "embed_dim").as_deref(),
            Some("16")
        );
        assert_eq!(mem.retrieve("berlin", 5).len(), 1);
        cleanup(dir);
    }

    #[cfg(not(feature = "embed"))]
    #[test]
    fn test_disabled_embedder_skips_vectors() {
        let (dir, mem) = temp_memory_with("disabled", Arc::new(crate::embed::DisabledEmbedder));
        mem.add("user lives in berlin".to_string(), vec![], None)
            .unwrap();
        assert_eq!(mem.retrieve("berlin", 5).len(), 0);
        let conn = mem.store.conn.lock().unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('memory_vec','transcript_vec')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "no vector tables without an embedder");
        drop(conn);
        cleanup(dir);
    }

    fn vector_count(mem: &Memory, table: &str) -> i64 {
        mem.store
            .conn
            .lock()
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn test_fragment_short_unchanged() {
        assert_eq!(
            fragment("user prefers dark mode", "dark"),
            "user prefers dark mode"
        );
        assert_eq!(fragment("a  b\nc", "b"), "a b c");
    }

    #[test]
    fn test_fragment_match_near_start() {
        let text = format!("needle {}", "x".repeat(200));
        let f = fragment(&text, "needle");
        assert!(f.contains("needle"), "{f}");
        assert!(f.ends_with('…'));
        assert!(!f.starts_with('…'));
        assert!(f.chars().count() <= FRAGMENT_CHARS, "{}", f.chars().count());
    }

    #[test]
    fn test_fragment_match_in_middle() {
        let text = format!("{} needle {}", "x".repeat(200), "y".repeat(200));
        let f = fragment(&text, "needle");
        assert!(f.contains("needle"), "{f}");
        assert!(f.starts_with('…'));
        assert!(f.ends_with('…'));
        assert!(f.chars().count() <= FRAGMENT_CHARS, "{}", f.chars().count());
    }

    #[test]
    fn test_fragment_no_literal_match() {
        let f = fragment(&"x".repeat(250), "absent");
        assert!(f.ends_with('…'));
        assert!(!f.contains("absent"));
        assert!(f.chars().count() <= FRAGMENT_CHARS);
    }

    #[test]
    fn test_fragment_boundary() {
        let exact = "a".repeat(FRAGMENT_CHARS);
        assert_eq!(fragment(&exact, "a"), exact);
        let over = "a".repeat(FRAGMENT_CHARS + 1);
        assert!(fragment(&over, "a").chars().count() <= FRAGMENT_CHARS);
    }

    #[test]
    fn test_fragment_multibyte_safe() {
        let text = format!("{} needle {}", "😀".repeat(120), "漢".repeat(120));
        let f = fragment(&text, "needle");
        assert!(f.contains("needle"), "{f}");
        assert!(f.chars().count() <= FRAGMENT_CHARS, "{}", f.chars().count());
    }

    #[test]
    fn test_unprocessed_batches_group_by_session() {
        let (dir, mem) = temp_memory("batches");
        let a: Vec<_> = (1..=12).map(|i| tuple(i, &format!("u{i}"), "a")).collect();
        let b: Vec<_> = (1..=3).map(|i| tuple(i, &format!("v{i}"), "b")).collect();
        mem.index_session("a", &a).unwrap();
        mem.index_session("b", &b).unwrap();
        let batches = mem.unprocessed_batches(10);
        let sizes: Vec<usize> = batches.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![10, 2, 3]);
        assert!(
            batches
                .iter()
                .all(|batch| batch.iter().all(|row| row.session == batch[0].session))
        );
        cleanup(dir);
    }

    #[test]
    fn test_reload_persists() {
        let (dir, mem) = temp_memory("reload");
        let key = mem
            .add("persisted".to_string(), vec!["kw".to_string()], None)
            .unwrap()
            .0;
        drop(mem);
        let reloaded = Memory::open(&dir.join("memory.db"), test_embedder()).unwrap();
        let hits = reloaded.retrieve("kw", 5);
        assert_eq!(hits[0].key, key);
        cleanup(dir);
    }
}
