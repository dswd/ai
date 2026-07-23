use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::SystemTime;

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
    use std::time::UNIX_EPOCH;
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let tm = secs_to_utc(secs);
    let millis = duration.subsec_millis();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        tm.year, tm.month, tm.day, tm.hour, tm.min, tm.sec, millis
    )
}

struct UtcTime {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
}

fn secs_to_utc(secs: u64) -> UtcTime {
    let days = secs / 86400;
    let time = secs % 86400;

    let hour = (time / 3600) as u32;
    let min = ((time % 3600) / 60) as u32;
    let sec = (time % 60) as u32;

    let mut year = 1970i64;
    let mut remaining_days = days as i64;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let days_per_month = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 0u32;
    for (i, &dpm) in days_per_month.iter().enumerate() {
        if remaining_days < dpm as i64 {
            month = i as u32 + 1;
            remaining_days += 1;
            break;
        }
        remaining_days -= dpm as i64;
    }

    UtcTime {
        year,
        month,
        day: remaining_days as u32,
        hour,
        min,
        sec,
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
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
