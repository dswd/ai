use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    #[serde(alias = "created_at")]
    pub created: String,
    #[serde(alias = "updated_at")]
    pub updated: String,
    pub system_prompt: String,
    pub model: String,
    pub messages: Vec<Message>,
}

impl Session {
    pub fn new(name: String, system_prompt: String, model: String) -> Self {
        let now = now_iso();
        Self {
            name,
            created: now.clone(),
            updated: now,
            system_prompt,
            model,
            messages: Vec::new(),
        }
    }

    pub fn add_message(&mut self, role: Role, content: &str) {
        self.messages.push(Message {
            role,
            content: content.to_string(),
        });
        self.updated = now_iso();
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.json", self.name));
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)
            .with_context(|| format!("saving session: {}", path.display()))?;
        Ok(())
    }

    pub fn load(name: &str, dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join(format!("{}.json", name));
        let json = std::fs::read_to_string(&path)
            .with_context(|| format!("loading session: {}", path.display()))?;
        let session: Session = serde_json::from_str(&json)
            .with_context(|| format!("parsing session: {}", path.display()))?;
        Ok(session)
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

fn now_iso() -> String {
    use time::OffsetDateTime;
    use time::format_description::FormatItem;
    use time::macros::format_description;

    let now = OffsetDateTime::now_utc();
    let fmt: &[FormatItem] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    now.format(fmt).unwrap_or_default()
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
    fn test_session_message_json_backward_compat() {
        // Session files written before the Role enum stored roles as lowercase strings.
        let msg: Message = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, "hi");
        let back = serde_json::to_string(&msg).unwrap();
        assert!(back.contains(r#""role":"user""#));
    }

    #[test]
    fn test_session_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ai-session-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::new("t".to_string(), "sys".to_string(), "m".to_string());
        s.add_message(Role::User, "hello");
        s.add_message(Role::Assistant, "hi there");
        s.save(&dir).unwrap();
        let loaded = Session::load("t", &dir).unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].role, Role::User);
        assert_eq!(loaded.messages[1].role, Role::Assistant);
        assert_eq!(loaded.messages[1].content, "hi there");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
