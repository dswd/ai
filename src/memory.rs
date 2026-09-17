use anyhow::Context;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rand::RngExt;

use crate::session::TranscriptTuple;

pub const TOP_K: usize = 5;
const MAX_ENTRIES: usize = 1000;
const UPSERT_JACCARD: f64 = 0.7;
const UPSERT_MIN_SHARED: usize = 2;
const RRF_K: f64 = 60.0;
const MEMORY_WEIGHT: f64 = 1.0;
const TRANSCRIPT_WEIGHT: f64 = 0.5;
const TRANSCRIPT_CAP: usize = 3;
const MAX_MATCH_TERMS: usize = 8;
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

CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
  text, tags, content='memory', content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS memory_ai AFTER INSERT ON memory BEGIN
  INSERT INTO memory_fts(rowid, text, tags) VALUES (new.rowid, new.text, new.tags);
END;
CREATE TRIGGER IF NOT EXISTS memory_ad AFTER DELETE ON memory BEGIN
  INSERT INTO memory_fts(memory_fts, rowid, text, tags)
    VALUES ('delete', old.rowid, old.text, old.tags);
END;
CREATE TRIGGER IF NOT EXISTS memory_au AFTER UPDATE ON memory
WHEN old.text IS NOT new.text OR old.tags IS NOT new.tags
BEGIN
  INSERT INTO memory_fts(memory_fts, rowid, text, tags)
    VALUES ('delete', old.rowid, old.text, old.tags);
  INSERT INTO memory_fts(rowid, text, tags) VALUES (new.rowid, new.text, new.tags);
END;

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

CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
  user_text, agent_text, content='transcripts', content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS transcripts_ai AFTER INSERT ON transcripts BEGIN
  INSERT INTO transcripts_fts(rowid, user_text, agent_text)
    VALUES (new.rowid, new.user_text, new.agent_text);
END;
CREATE TRIGGER IF NOT EXISTS transcripts_ad AFTER DELETE ON transcripts BEGIN
  INSERT INTO transcripts_fts(transcripts_fts, rowid, user_text, agent_text)
    VALUES ('delete', old.rowid, old.user_text, old.agent_text);
END;
CREATE TRIGGER IF NOT EXISTS transcripts_au AFTER UPDATE ON transcripts
WHEN old.user_text IS NOT new.user_text OR old.agent_text IS NOT new.agent_text
BEGIN
  INSERT INTO transcripts_fts(transcripts_fts, rowid, user_text, agent_text)
    VALUES ('delete', old.rowid, old.user_text, old.agent_text);
  INSERT INTO transcripts_fts(rowid, user_text, agent_text)
    VALUES (new.rowid, new.user_text, new.agent_text);
END;
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
    pub fn open(path: &Path) -> anyhow::Result<Self> {
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
        Ok(Self {
            store: Arc::new(Store {
                conn: Mutex::new(conn),
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
        Ok((id, false))
    }

    pub fn delete(&self, key: &str) -> Result<String, String> {
        let conn = self.store.conn.lock().unwrap();
        let changed = conn
            .execute("DELETE FROM memory WHERE id=?1", params![key])
            .map_err(|e| format!("failed to delete memory: {e}"))?;
        if changed == 0 {
            return Err(format!("No memory entry with key '{key}'."));
        }
        Ok(format!("Deleted memory entry '{key}'."))
    }

    #[cfg(test)]
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

    /// Combined memory + transcript retrieval, fused with reciprocal rank fusion.
    /// Transcript hits are capped so memory keeps the majority of the slots.
    pub fn retrieve(&self, query: &str, top_k: usize) -> Vec<Hit> {
        let Some(match_expr) = build_match(query) else {
            return Vec::new();
        };
        let conn = self.store.conn.lock().unwrap();
        let fetch = top_k.max(1) * OVERFETCH;
        let mut fused: HashMap<String, (Hit, f64)> = HashMap::new();

        if let Ok(mut stmt) = conn.prepare(
            "SELECT m.id, m.text, m.tags, m.created, bm25(memory_fts)
             FROM memory_fts JOIN memory m ON m.rowid = memory_fts.rowid
             WHERE memory_fts MATCH ?1 ORDER BY bm25(memory_fts) ASC LIMIT ?2",
        ) {
            let rows = stmt
                .query_map(params![match_expr, fetch as i64], |r| {
                    let tags: String = r.get(2)?;
                    Ok(Hit {
                        kind: HitKind::Memory,
                        key: r.get(0)?,
                        text: r.get(1)?,
                        tags: parse_tags(&tags),
                        session: None,
                        created: r.get(3)?,
                    })
                })
                .map(|rows| rows.flatten().collect::<Vec<_>>())
                .unwrap_or_default();
            for (rank, hit) in rows.into_iter().enumerate() {
                let score = MEMORY_WEIGHT / (RRF_K + (rank + 1) as f64);
                fused.insert(format!("m:{}", hit.key), (hit, score));
            }
        }

        if let Ok(mut stmt) = conn.prepare(
            "SELECT t.session, t.seq, t.user_text, t.agent_text, t.created, bm25(transcripts_fts)
             FROM transcripts_fts JOIN transcripts t ON t.rowid = transcripts_fts.rowid
             WHERE transcripts_fts MATCH ?1 ORDER BY bm25(transcripts_fts) ASC LIMIT ?2",
        ) {
            let rows = stmt
                .query_map(params![match_expr, fetch as i64], |r| {
                    let seq: i64 = r.get(1)?;
                    let user: String = r.get(2)?;
                    let agent: String = r.get(3)?;
                    let session: String = r.get(0)?;
                    Ok(Hit {
                        kind: HitKind::Transcript,
                        key: format!("{session}#{seq}"),
                        text: format!("user: {user}\nagent: {agent}"),
                        tags: Vec::new(),
                        session: Some(session),
                        created: r.get(4)?,
                    })
                })
                .map(|rows| rows.flatten().collect::<Vec<_>>())
                .unwrap_or_default();
            for (rank, hit) in rows.into_iter().enumerate() {
                let score = TRANSCRIPT_WEIGHT / (RRF_K + (rank + 1) as f64);
                fused.insert(format!("t:{}", hit.key), (hit, score));
            }
        }

        let mut ranked: Vec<(Hit, f64)> = fused.into_values().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut out: Vec<Hit> = Vec::new();
        let mut transcripts = 0usize;
        for (hit, _) in ranked {
            if hit.kind == HitKind::Transcript {
                if transcripts >= TRANSCRIPT_CAP {
                    continue;
                }
                transcripts += 1;
            }
            out.push(hit);
            if out.len() >= top_k {
                break;
            }
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

    /// Store one session's user+agent tuples. Rewrites rows whose text changed
    /// (resetting `processed`) and drops rows beyond the current log, so a
    /// cleared or compacted session is reflected too.
    pub fn index_session(
        &self,
        session: &str,
        tuples: &[TranscriptTuple],
    ) -> anyhow::Result<usize> {
        let mut conn = self.store.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = now_iso();
        let mut changed = 0;
        for t in tuples {
            changed += tx.execute(
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
        let max_seq = tuples.last().map(|t| t.seq as i64).unwrap_or(0);
        tx.execute(
            "DELETE FROM transcripts WHERE session=?1 AND seq > ?2",
            params![session, max_seq],
        )?;
        tx.commit()?;
        Ok(changed)
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

    /// Similar entries created after `entry`, as supersession evidence.
    pub fn similar_newer(&self, entry: &MemoryEntry, limit: usize) -> Vec<MemoryEntry> {
        let Some(match_expr) = build_match(&entry.text) else {
            return Vec::new();
        };
        let conn = self.store.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT m.id, m.text, m.tags, m.created, m.updated, m.last_used, m.last_judged, m.origin, m.source_session
             FROM memory_fts JOIN memory m ON m.rowid = memory_fts.rowid
             WHERE memory_fts MATCH ?1 AND m.id != ?2 AND m.updated > ?3
             ORDER BY m.updated DESC LIMIT ?4",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(
            params![match_expr, entry.id, entry.updated, limit as i64],
            map_entry,
        )
        .map(|rows| rows.flatten().collect())
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

/// Build a safe FTS5 `MATCH` expression from a user query: tokenized, stopword
/// filtered, deduplicated, and quoted so operators or stray quotes cannot reach
/// the FTS parser.
fn build_match(query: &str) -> Option<String> {
    let terms = tokenize(query);
    if terms.is_empty() {
        return None;
    }
    let mut seen = HashSet::new();
    let mut parts = Vec::new();
    for term in terms {
        if seen.insert(term.clone()) {
            parts.push(format!("\"{term}\""));
            if parts.len() >= MAX_MATCH_TERMS {
                break;
            }
        }
    }
    Some(parts.join(" OR "))
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

    fn temp_memory(name: &str) -> (PathBuf, Memory) {
        let dir =
            std::env::temp_dir().join(format!("ai-memory-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.db");
        (dir, Memory::open(&path).unwrap())
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

        let mem = Memory::open(&dir.join("memory.db")).unwrap();
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

        let mem = Memory::open(&path).unwrap();
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
        let mem = Memory::open(&path).unwrap();
        assert_eq!(mem.list().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_match_sanitizes() {
        assert!(build_match("").is_none());
        assert!(build_match("the a of").is_none());
        let m = build_match("berlin\" OR NEAR(").unwrap();
        assert!(m.contains("\"berlin\""));
        assert!(!m.contains("NEAR"));
        assert!(!m.contains('('));
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
        let reloaded = Memory::open(&dir.join("memory.db")).unwrap();
        let hits = reloaded.retrieve("kw", 5);
        assert_eq!(hits[0].key, key);
        cleanup(dir);
    }
}
