use anyhow::Context;
use rig::completion::message::UserContent;
use rig::completion::{AssistantContent, Message as ChatMessage};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::SystemTime;

/// A fresh session is resumed when its file was modified within this window.
pub const RESUME_WINDOW_SECS: u64 = 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

/// A message as persisted by session schema v1 (role + text only). Retained to
/// migrate old session files; new sessions store the full chat log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacySession {
    pub name: String,
    #[serde(alias = "created_at")]
    pub created: String,
    #[serde(alias = "updated_at")]
    pub updated: String,
    pub system_prompt: String,
    pub model: String,
    pub messages: Vec<LegacyMessage>,
}

/// One completed user/agent exchange, as indexed for transcript search and
/// dreaming. Tool calls and tool results are not represented; they stay in the
/// session's full log for resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptTuple {
    /// Ordinal of the user turn within the session (1-based).
    pub seq: usize,
    pub user: String,
    pub agent: String,
}

/// A saved session. `log` is the complete, provider-agnostic chat message list
/// (including tool calls and tool results) so a resume replays the exact context
/// the model saw. `messages` is only populated when migrating a v1 file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    #[serde(alias = "created_at")]
    pub created: String,
    #[serde(alias = "updated_at")]
    pub updated: String,
    pub system_prompt: String,
    pub model: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub log: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<LegacyMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    /// True when loaded from a v1 file whose tool history is unavailable.
    #[serde(skip)]
    pub partial: bool,
}

pub const SESSION_VERSION: u32 = 2;

/// A session name must be a single path component so it cannot escape the
/// session directory (`--session-name=../../x`, `session delete ../foo`).
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

impl Session {
    pub fn new(name: String, system_prompt: String, model: String, provider: String) -> Self {
        let now = crate::util::now_iso();
        Self {
            name,
            created: now.clone(),
            updated: now,
            system_prompt,
            model,
            provider,
            version: SESSION_VERSION,
            log: Vec::new(),
            messages: Vec::new(),
            forked_from: None,
            partial: false,
        }
    }

    /// Append a full chat message (text, tool call, or tool result) to the log.
    pub fn push_message(&mut self, message: ChatMessage) {
        self.log.push(message);
        self.updated = crate::util::now_iso();
    }

    /// Extend the log with a completed run's messages.
    pub fn extend_messages(&mut self, messages: impl IntoIterator<Item = ChatMessage>) {
        self.log.extend(messages);
        self.updated = crate::util::now_iso();
    }

    pub fn add_user(&mut self, text: &str) {
        self.push_message(ChatMessage::user(text));
    }

    pub fn add_assistant(&mut self, text: &str) {
        self.push_message(ChatMessage::assistant(text));
    }

    pub fn add_system(&mut self, text: &str) {
        self.push_message(ChatMessage::system(text));
    }

    /// The full chat history for replay.
    pub fn chat_history(&self) -> Vec<ChatMessage> {
        self.log.clone()
    }

    /// The user/assistant/system text view for display, prompts, and memory
    /// reconciliation. Tool-call and tool-result sides are omitted.
    pub fn transcript(&self) -> Vec<(Role, String)> {
        self.log
            .iter()
            .filter_map(|m| match m {
                ChatMessage::System { content } => Some((Role::System, content.clone())),
                ChatMessage::User { content } => {
                    let mut text = String::new();
                    for item in content.iter() {
                        if let UserContent::Text(t) = item {
                            text.push_str(&t.text);
                        }
                    }
                    (!text.is_empty()).then_some((Role::User, text))
                }
                ChatMessage::Assistant { content, .. } => {
                    let mut text = String::new();
                    for item in content.iter() {
                        if let AssistantContent::Text(t) = item {
                            text.push_str(&t.text);
                        }
                    }
                    (!text.is_empty()).then_some((Role::Assistant, text))
                }
            })
            .collect()
    }

    /// Completed user/agent exchanges in order. Each tuple is a user turn plus
    /// the text the agent produced for it; intermediate tool traffic is dropped.
    /// A trailing user turn without an agent reply yet is not emitted.
    pub fn tuples(&self) -> Vec<TranscriptTuple> {
        let mut tuples = Vec::new();
        let mut ordinal = 0usize;
        let mut pending: Option<(usize, String)> = None;
        let mut agent = String::new();
        let flush = |pending: &mut Option<(usize, String)>,
                     agent: &mut String,
                     tuples: &mut Vec<TranscriptTuple>| {
            if let Some((seq, user)) = pending.take() {
                let agent = agent.trim();
                if !agent.is_empty() {
                    tuples.push(TranscriptTuple {
                        seq,
                        user,
                        agent: agent.to_string(),
                    });
                }
            }
            agent.clear();
        };
        for (role, text) in self.transcript() {
            match role {
                Role::User => {
                    flush(&mut pending, &mut agent, &mut tuples);
                    ordinal += 1;
                    pending = Some((ordinal, text));
                }
                Role::Assistant => {
                    if pending.is_some() {
                        if !agent.is_empty() {
                            agent.push('\n');
                        }
                        agent.push_str(&text);
                    }
                }
                Role::System => {}
            }
        }
        flush(&mut pending, &mut agent, &mut tuples);
        tuples
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        if !is_safe_name(&self.name) {
            anyhow::bail!("invalid session name: {:?}", self.name);
        }
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.json", self.name));
        let mut to_save = self.clone();
        to_save.version = SESSION_VERSION;
        to_save.messages = Vec::new();
        to_save.partial = false;
        let json = serde_json::to_string_pretty(&to_save)?;
        std::fs::write(&path, json)
            .with_context(|| format!("saving session: {}", path.display()))?;
        Ok(())
    }

    pub fn load(name: &str, dir: &Path) -> anyhow::Result<Self> {
        if !is_safe_name(name) {
            anyhow::bail!("invalid session name: {name:?}");
        }
        let path = dir.join(format!("{}.json", name));
        let json = std::fs::read_to_string(&path)
            .with_context(|| format!("loading session: {}", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&json)
            .with_context(|| format!("parsing session: {}", path.display()))?;

        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        if version >= SESSION_VERSION as u64 {
            let session: Session = serde_json::from_value(value)
                .with_context(|| format!("parsing session: {}", path.display()))?;
            Ok(session)
        } else {
            let legacy: LegacySession = serde_json::from_value(value)
                .with_context(|| format!("parsing legacy session: {}", path.display()))?;
            let log = legacy
                .messages
                .iter()
                .map(|m| match m.role {
                    Role::User => ChatMessage::user(&m.content),
                    Role::Assistant => ChatMessage::assistant(&m.content),
                    Role::System => ChatMessage::system(&m.content),
                })
                .collect();
            Ok(Session {
                name: legacy.name,
                created: legacy.created,
                updated: legacy.updated,
                system_prompt: legacy.system_prompt,
                model: legacy.model,
                provider: String::new(),
                version: SESSION_VERSION,
                log,
                messages: Vec::new(),
                forked_from: None,
                partial: true,
            })
        }
    }

    pub fn list(dir: &Path) -> anyhow::Result<Vec<String>> {
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }
}

/// The `YYYY-MM-DD` UTC date prefix for generated session names.
fn today_prefix() -> String {
    use time::OffsetDateTime;
    use time::macros::format_description;

    OffsetDateTime::now_utc()
        .format(format_description!("[year]-[month]-[day]"))
        .unwrap_or_default()
}

/// `YYYY-MM-DD_name`: prefix an explicit base with today's date.
pub fn dated_name(base: &str) -> String {
    format!("{}_{base}", today_prefix())
}

/// True for a name of the form `YYYY-MM-DD_<rest>`.
pub fn is_dated(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() > 11
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'_'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| b[i].is_ascii_digit())
}

/// True when `stem` is the base itself or a date-prefixed form of it.
fn matches_base(stem: &str, base: &str) -> bool {
    if stem == base {
        return true;
    }
    is_dated(stem) && &stem[11..] == base
}

/// The most recently modified `*.json` session whose name is `base` itself or a
/// date-prefixed variant of it (`YYYY-MM-DD_base`).
pub fn find_named(dir: &Path, base: &str) -> Option<String> {
    let mut best: Option<(String, SystemTime)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !matches_base(stem, base) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, t)| modified > *t) {
            best = Some((stem.to_string(), modified));
        }
    }
    best.map(|(name, _)| name)
}

/// Generate a unique session name `YYYY-MM-DD_<adj>-<noun>` in `dir`.
pub fn generate_session_name(dir: &Path) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let adjectives = [
        "swift", "calm", "bright", "keen", "bold", "wise", "warm", "cool", "fair", "fine", "glad",
        "pure", "rare", "safe", "wild", "deep", "eager", "fresh", "grand", "happy", "jolly",
        "light", "merry", "noble", "proud", "quiet", "sharp", "sunny", "vivid", "zesty",
    ];

    let nouns = [
        "hawk", "wolf", "bear", "deer", "dove", "fox", "lark", "lynx", "owl", "seal", "swan",
        "wren", "fern", "oak", "pine", "rose", "coral", "crane", "finch", "heron", "ibis", "jay",
        "kiwi", "newt", "pika", "tiger", "trout", "whale", "zebra", "falcon",
    ];

    let date = today_prefix();
    for attempt in 0..1000u64 {
        let n = seed.wrapping_add(attempt);
        let adj = adjectives[(n as usize) % adjectives.len()];
        let noun = nouns[(n.wrapping_mul(7) as usize) % nouns.len()];
        let name = format!("{date}_{adj}-{noun}");
        if !dir.join(format!("{name}.json")).exists() {
            return name;
        }
    }
    format!("{date}_{seed}")
}

/// The `*.json` session with the greatest modification time, with that time.
pub fn newest(dir: &Path) -> Option<(String, SystemTime)> {
    let mut best: Option<(String, SystemTime)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, t)| modified > *t) {
            best = Some((name.to_string(), modified));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_safe_name() {
        assert!(is_safe_name("calm-hawk"));
        assert!(is_safe_name("my.session"));
        assert!(!is_safe_name(""));
        assert!(!is_safe_name("."));
        assert!(!is_safe_name(".."));
        assert!(!is_safe_name("../foo"));
        assert!(!is_safe_name("a/b"));
        assert!(!is_safe_name("a\\b"));
    }

    #[test]
    fn test_load_rejects_unsafe_name() {
        let dir = std::env::temp_dir().join(format!("ai-session-safe-{}", std::process::id()));
        let err = Session::load("../escape", &dir).unwrap_err();
        assert!(err.to_string().contains("invalid session name"));
    }

    #[test]
    fn test_role_serde_roundtrip() {
        for (role, json) in [
            (Role::User, "\"user\""),
            (Role::Assistant, "\"assistant\""),
            (Role::System, "\"system\""),
        ] {
            let serialized = serde_json::to_string(&role).unwrap();
            assert_eq!(serialized, json);
            let parsed: Role = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, role);
        }
    }

    #[test]
    fn test_session_v2_roundtrip_preserves_full_log() {
        let dir = std::env::temp_dir().join(format!("ai-session-v2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::new(
            "t".to_string(),
            "sys".to_string(),
            "m".to_string(),
            "openai".to_string(),
        );
        s.add_user("hello");
        s.push_message(ChatMessage::Assistant {
            id: Some("id1".to_string()),
            content: vec![AssistantContent::text("calling")],
        });
        s.push_message(ChatMessage::tool_result("id1", "tool", "tool output"));
        s.add_assistant("done");
        s.save(&dir).unwrap();

        let loaded = Session::load("t", &dir).unwrap();
        assert_eq!(loaded.log.len(), 4, "tool call and result must persist");
        assert!(!loaded.partial);
        assert_eq!(loaded.provider, "openai");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_session_v1_migration_marked_partial() {
        let dir = std::env::temp_dir().join(format!("ai-session-v1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = r#"{
            "name": "old",
            "created": "2020-01-01T00:00:00Z",
            "updated": "2020-01-01T00:00:00Z",
            "system_prompt": "sys",
            "model": "m",
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "there"}
            ]
        }"#;
        std::fs::write(dir.join("old.json"), legacy).unwrap();
        let loaded = Session::load("old", &dir).unwrap();
        assert!(loaded.partial, "v1 sessions have no tool history");
        assert_eq!(loaded.log.len(), 2);
        assert_eq!(loaded.transcript().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_transcript_omits_tool_messages() {
        let mut s = Session::new(
            "t".to_string(),
            "sys".to_string(),
            "m".to_string(),
            "openai".to_string(),
        );
        s.add_user("hello");
        s.push_message(ChatMessage::Assistant {
            id: Some("id1".to_string()),
            content: vec![AssistantContent::text("call")],
        });
        s.push_message(ChatMessage::tool_result("id1", "tool", "secret"));
        s.add_assistant("answer");
        let t = s.transcript();
        assert_eq!(t.len(), 3);
        assert!(t.iter().any(|(r, c)| *r == Role::User && c == "hello"));
        assert!(!t.iter().any(|(_, c)| c.contains("secret")));
    }

    fn set_mtime(path: &Path, when: SystemTime) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn test_generate_session_name_is_date_prefixed_and_unique() {
        let dir = std::env::temp_dir().join(format!("ai-name-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let prefix = format!("{}_", today_prefix());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let name = generate_session_name(&dir);
            assert!(name.starts_with(&prefix), "name {name}");
            assert_eq!(
                name.trim_start_matches(&*prefix).matches('-').count(),
                1,
                "name {name}"
            );
            assert!(seen.insert(name.clone()), "duplicate name {name}");
            std::fs::write(dir.join(format!("{name}.json")), "{}").unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_newest_picks_max_mtime_and_ignores_non_json() {
        let dir = std::env::temp_dir().join(format!("ai-newest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let old = dir.join("old.json");
        let new = dir.join("new.json");
        std::fs::write(&old, "{}").unwrap();
        std::fs::write(&new, "{}").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();

        set_mtime(
            &old,
            SystemTime::now() - std::time::Duration::from_secs(3600),
        );
        set_mtime(&new, SystemTime::now());

        assert_eq!(newest(&dir).unwrap().0, "new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_dated_and_matches_base() {
        assert!(is_dated("2026-09-15_calm-hawk"));
        assert!(!is_dated("calm-hawk"));
        assert!(!is_dated("2026-09-15"));
        assert!(!is_dated("2026-9-15_calm"));
        assert!(!is_dated("abcd-09-15_calm"));

        assert!(matches_base("foo", "foo"));
        assert!(matches_base("2026-01-01_foo", "foo"));
        assert!(!matches_base("2026-01-01_foobar", "foo"));
        assert!(!matches_base("2026-01-01_foo_bar", "foo"));
    }

    #[test]
    fn test_find_named_picks_latest_match() {
        let dir = std::env::temp_dir().join(format!("ai-findnamed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let newer = dir.join("2026-01-01_foo.json");
        let older = dir.join("2026-09-15_foo.json");
        for path in [&newer, &older] {
            std::fs::write(path, "{}").unwrap();
        }
        std::fs::write(dir.join("other.json"), "{}").unwrap();

        set_mtime(
            &older,
            SystemTime::now() - std::time::Duration::from_secs(3600),
        );
        set_mtime(&newer, SystemTime::now());

        assert_eq!(find_named(&dir, "foo").as_deref(), Some("2026-01-01_foo"));
        assert_eq!(find_named(&dir, "nope"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tuples_pair_exchanges() {
        let mut s = Session::new("t".into(), "sys".into(), "m".into(), "openai".into());
        s.add_user("first");
        s.add_assistant("answer one");
        s.add_user("second");
        s.add_assistant("answer two");
        let tuples = s.tuples();
        assert_eq!(tuples.len(), 2);
        assert_eq!(tuples[0].seq, 1);
        assert_eq!(tuples[0].user, "first");
        assert_eq!(tuples[0].agent, "answer one");
        assert_eq!(tuples[1].seq, 2);
    }

    #[test]
    fn test_tuples_skip_unreplied_turn() {
        let mut s = Session::new("t".into(), "sys".into(), "m".into(), "openai".into());
        s.add_user("answered");
        s.add_assistant("reply");
        s.add_user("still waiting");
        let tuples = s.tuples();
        assert_eq!(tuples.len(), 1, "unreplied trailing turn is not emitted");
    }
}
