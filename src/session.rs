use anyhow::Context;
use rig_core::completion::message::UserContent;
use rig_core::completion::{AssistantContent, Message as ChatMessage};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
    #[serde(default)]
    pub reconciled_until: usize,
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
    #[serde(default)]
    pub reconciled_until: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    /// True when loaded from a v1 file whose tool history is unavailable.
    #[serde(skip)]
    pub partial: bool,
}

pub const SESSION_VERSION: u32 = 2;

/// Extract user-authored text from a message. Returns `None` for tool-result
/// messages (which are also `Message::User`) and non-text content.
pub fn user_text(msg: &ChatMessage) -> Option<String> {
    if let ChatMessage::User { content } = msg {
        let mut text = String::new();
        for item in content.iter() {
            if let UserContent::Text(t) = item {
                text.push_str(&t.text);
            }
        }
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
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
            reconciled_until: 0,
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

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
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
                reconciled_until: legacy.reconciled_until,
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

pub fn generate_session_name() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();

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

    let adj = adjectives[(nanos as usize) % adjectives.len()];
    let noun = nouns[(nanos.wrapping_mul(7) as usize) % nouns.len()];
    format!("{adj}-{noun}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        s.push_message(ChatMessage::assistant_with_id("id1".to_string(), "calling"));
        s.push_message(ChatMessage::tool_result("id1", "tool output"));
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
        s.push_message(ChatMessage::assistant_with_id("id1".to_string(), "call"));
        s.push_message(ChatMessage::tool_result("id1", "secret"));
        s.add_assistant("answer");
        let t = s.transcript();
        assert_eq!(t.len(), 3);
        assert!(t.iter().any(|(r, c)| *r == Role::User && c == "hello"));
        assert!(!t.iter().any(|(_, c)| c.contains("secret")));
    }
}
