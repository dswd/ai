use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rand::RngExt;

const MAX_ENTRIES: usize = 1000;
pub const TOP_K: usize = 5;
const MIN_SCORE: f64 = 0.2;
const UPSERT_SCORE: f64 = 0.7;
const KEYWORD_BONUS: f64 = 2.0;
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

static STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "he", "in", "is", "it",
    "its", "of", "on", "that", "the", "to", "was", "were", "will", "with", "what", "when", "where",
    "who", "how", "i", "you", "your", "we", "our", "they", "their", "do", "does", "did", "not",
    "no", "yes", "me", "my", "this", "these", "those",
];

fn tokenize(text: &str) -> Vec<String> {
    static TOKEN_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = TOKEN_RE.get_or_init(|| Regex::new(r"[^a-zA-Z0-9]+").unwrap());
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();
    re.split(text)
        .map(|s| s.to_lowercase())
        .filter(|s| !s.is_empty() && !stopwords.contains(s.as_str()))
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub created: String,
    pub updated: String,
    #[serde(default)]
    pub source_session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryFile {
    version: u32,
    entries: Vec<MemoryEntry>,
}

#[derive(Debug)]
pub struct Memory {
    path: PathBuf,
    session_name: Mutex<Option<String>>,
    entries: Mutex<Vec<MemoryEntry>>,
}

fn now_iso() -> String {
    use time::OffsetDateTime;
    use time::format_description::FormatItem;
    use time::macros::format_description;

    let now = OffsetDateTime::now_utc();
    let fmt: &[FormatItem] = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    now.format(fmt).unwrap_or_default()
}

impl Memory {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let entries = if path.exists() {
            let content = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("reading memory file {}: {e}", path.display()))?;
            parse_memory_file(&content)
                .map_err(|e| anyhow::anyhow!("parsing memory file {}: {e}", path.display()))?
        } else {
            Vec::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            session_name: Mutex::new(None),
            entries: Mutex::new(entries),
        })
    }

    pub fn set_session_name(&self, name: &str) {
        *self.session_name.lock().unwrap() = Some(name.to_string());
    }

    fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let entries = self.entries.lock().unwrap();
        let file = MemoryFile {
            version: 2,
            entries: entries.clone(),
        };
        let json = serde_json::to_string_pretty(&file)?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }

    pub fn add(&self, text: String, keywords: Vec<String>) -> Result<String, String> {
        let source = self.session_name.lock().unwrap().clone();
        self.add_with_source(text, keywords, source.as_deref())
            .map(|(id, _)| id)
    }

    /// Store a memory entry. If a near-duplicate exists it is updated in place;
    /// returns the entry id and whether it updated an existing entry.
    pub fn add_with_source(
        &self,
        text: String,
        keywords: Vec<String>,
        source: Option<&str>,
    ) -> Result<(String, bool), String> {
        let mut entries = self.entries.lock().unwrap();
        let keywords: Vec<String> = keywords
            .iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();

        if !entries.is_empty() {
            let query = tokenize(&text);
            if !query.is_empty() {
                let avgdl = avg_doc_len(&entries);
                let mut best: Option<(usize, f64)> = None;
                for i in 0..entries.len() {
                    let score = bm25_score(&entries, i, &query, avgdl);
                    if best.is_none_or(|(_, bs)| score > bs) {
                        best = Some((i, score));
                    }
                }
                if let Some((i, score)) = best
                    && score >= UPSERT_SCORE
                {
                    let entry = &mut entries[i];
                    let id = entry.id.clone();
                    entry.text = text;
                    entry.keywords = keywords;
                    entry.updated = now_iso();
                    if let Some(s) = source {
                        entry.source_session = Some(s.to_string());
                    }
                    drop(entries);
                    self.save()
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
        entries.push(MemoryEntry {
            id: id.clone(),
            text,
            keywords,
            created: now.clone(),
            updated: now,
            source_session: source.map(str::to_string),
        });
        drop(entries);
        self.save()
            .map_err(|e| format!("failed to save memory: {e}"))?;
        Ok((id, false))
    }

    pub fn delete(&self, key: &str) -> Result<String, String> {
        let mut entries = self.entries.lock().unwrap();
        let before = entries.len();
        entries.retain(|e| e.id != key);
        if entries.len() == before {
            return Err(format!("No memory entry with key '{key}'."));
        }
        drop(entries);
        self.save()
            .map_err(|e| format!("failed to save memory: {e}"))?;
        Ok(format!("Deleted memory entry '{key}'."))
    }

    pub fn retrieve(&self, query: &str, top_k: usize) -> Vec<MemoryEntry> {
        let entries = self.entries.lock().unwrap();
        let qterms = tokenize(query);
        if qterms.is_empty() || entries.is_empty() {
            return Vec::new();
        }
        let avgdl = avg_doc_len(&entries);
        let mut scored: Vec<(usize, f64)> = entries
            .iter()
            .enumerate()
            .map(|(i, _)| (i, bm25_score(&entries, i, &qterms, avgdl)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .filter(|(_, s)| *s >= MIN_SCORE)
            .take(top_k)
            .map(|(i, _)| entries[i].clone())
            .collect()
    }

    pub fn list(&self) -> Vec<MemoryEntry> {
        self.entries.lock().unwrap().clone()
    }

    pub fn summary(&self) -> String {
        let entries = self.entries.lock().unwrap();
        if entries.is_empty() {
            "## Memory\nMemory is enabled but empty. Relevant entries are injected per message; use memory_add to store facts (optionally with keywords).".to_string()
        } else {
            format!(
                "## Memory\n{} entries stored. Relevant entries are injected per message; use memory_add to store new facts (optionally with keywords).",
                entries.len()
            )
        }
    }
}

fn avg_doc_len(entries: &[MemoryEntry]) -> f64 {
    let total: usize = entries.iter().map(|e| tokenize(&e.text).len()).sum();
    (total as f64 / entries.len() as f64).max(1.0)
}

fn bm25_score(entries: &[MemoryEntry], idx: usize, qterms: &[String], avgdl: f64) -> f64 {
    let doc = &entries[idx];
    let terms = tokenize(&doc.text);
    let dl = terms.len() as f64;
    let n = entries.len() as f64;

    let mut score = 0.0;
    for qt in qterms {
        let df = entries
            .iter()
            .filter(|e| tokenize(&e.text).iter().any(|t| t == qt))
            .count() as f64;
        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
        let f = terms.iter().filter(|t| *t == qt).count() as f64;
        if f > 0.0 {
            let denom = f + BM25_K1 * (1.0 - BM25_B + BM25_B * (dl / avgdl));
            score += idf * (f * (BM25_K1 + 1.0)) / denom;
        }
        if doc
            .keywords
            .iter()
            .any(|k| tokenize(k).iter().any(|t| t == qt))
        {
            score += KEYWORD_BONUS;
        }
    }
    score
}

fn parse_memory_file(content: &str) -> anyhow::Result<Vec<MemoryEntry>> {
    let value: serde_json::Value = serde_json::from_str(content)?;
    if let Some(version) = value.get("version").and_then(|v| v.as_u64())
        && version >= 2
    {
        let file: MemoryFile = serde_json::from_value(value)?;
        return Ok(file.entries);
    }

    let map: HashMap<String, String> = serde_json::from_value(value)?;
    let now = now_iso();
    Ok(map
        .into_iter()
        .map(|(id, text)| MemoryEntry {
            id,
            text,
            keywords: Vec::new(),
            created: now.clone(),
            updated: now.clone(),
            source_session: None,
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
        let path = dir.join("memory.json");
        (dir, Memory::load(&path).unwrap())
    }

    fn cleanup(dir: PathBuf) {
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_empty_memory() {
        let (dir, mem) = temp_memory("empty");
        assert_eq!(mem.retrieve("anything", 5).len(), 0);
        assert!(mem.summary().contains("empty"));
        cleanup(dir);
    }

    #[test]
    fn test_add_and_retrieve_by_keywords() {
        let (dir, mem) = temp_memory("retrieve");
        mem.add(
            "user prefers dark mode".to_string(),
            vec!["dark mode".to_string(), "preference".to_string()],
        )
        .unwrap();
        let hits = mem.retrieve("dark mode please", 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].text, "user prefers dark mode");
        cleanup(dir);
    }

    #[test]
    fn test_retrieve_respects_min_score() {
        let (dir, mem) = temp_memory("minscore");
        mem.add(
            "the capital of france is paris".to_string(),
            vec!["france".to_string()],
        )
        .unwrap();
        let hits = mem.retrieve("quantum entanglement theory", 5);
        assert!(hits.is_empty());
        cleanup(dir);
    }

    #[test]
    fn test_keyword_boost_outweighs_low_text_overlap() {
        let (dir, mem) = temp_memory("keywordboost");
        mem.add(
            "user's favorite food".to_string(),
            vec!["pizza".to_string()],
        )
        .unwrap();
        mem.add(
            "some unrelated tech note".to_string(),
            vec!["rust".to_string()],
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
            .add_with_source(
                "user prefers dark mode".to_string(),
                vec!["dark mode".to_string()],
                None,
            )
            .unwrap();
        assert!(!updated1);
        let (id2, updated2) = mem
            .add_with_source(
                "the user prefers dark mode theme".to_string(),
                vec!["dark mode".to_string()],
                None,
            )
            .unwrap();
        assert_eq!(id1, id2, "near-duplicate should update the existing entry");
        assert!(updated2, "second add should report an update");
        let all = mem.retrieve("dark mode", 5);
        assert_eq!(all.len(), 1);
        assert!(all[0].text.contains("dark mode theme"));
        cleanup(dir);
    }

    #[test]
    fn test_delete() {
        let (dir, mem) = temp_memory("delete");
        let key = mem.add("data".to_string(), Vec::new()).unwrap();
        assert!(mem.delete(&key).is_ok());
        assert!(mem.delete(&key).is_err());
        cleanup(dir);
    }

    #[test]
    fn test_migrate_legacy_map() {
        let dir =
            std::env::temp_dir().join(format!("ai-memory-test-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.json");
        std::fs::write(&path, r#"{"a1b2": "legacy fact"}"#).unwrap();
        let mem = Memory::load(&path).unwrap();
        let all = mem.retrieve("legacy fact", 5);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "a1b2");
        assert_eq!(all[0].keywords.len(), 0);
        cleanup(dir);
    }

    #[test]
    fn test_reload_persists() {
        let (dir, mem) = temp_memory("reload");
        let path = dir.join("memory.json");
        let key = mem
            .add("persisted".to_string(), vec!["kw".to_string()])
            .unwrap();
        drop(mem);
        let reloaded = Memory::load(&path).unwrap();
        let all = reloaded.retrieve("kw", 5);
        assert!(!all.is_empty());
        assert_eq!(all[0].id, key);
        cleanup(dir);
    }

    #[test]
    fn test_add_with_source_session() {
        let (dir, mem) = temp_memory("source");
        mem.set_session_name("mysession");
        mem.add("fact".to_string(), Vec::new()).unwrap();
        let all = mem.retrieve("fact", 5);
        assert_eq!(all[0].source_session.as_deref(), Some("mysession"));
        cleanup(dir);
    }
}
