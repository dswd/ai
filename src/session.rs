use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;
use crate::util::now_iso;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
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

    pub fn add_message(&mut self, role: &str, content: &str) {
        self.messages.push(Message {
            role: role.to_string(),
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

    #[allow(dead_code)]
    pub fn list(dir: &Path) -> anyhow::Result<Vec<String>> {
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "json") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    names.push(name.to_string());
                }
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
