use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rand::RngExt;

const MAX_ENTRIES: usize = 100;

#[derive(Debug)]
pub struct Memory {
    path: PathBuf,
    entries: Mutex<HashMap<String, String>>,
}

impl Memory {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let entries = if path.exists() {
            let content =
                std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("reading memory file {}: {e}", path.display()))?;
            serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("parsing memory file {}: {e}", path.display()))?
        } else {
            HashMap::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            entries: Mutex::new(entries),
        })
    }

    fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let entries = self.entries.lock().unwrap();
        let json = serde_json::to_string_pretty(&*entries)?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }

    pub fn add(&self, data: String) -> Result<String, String> {
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= MAX_ENTRIES {
            return Err("Memory is full (max 100 entries). Delete some entries first.".to_string());
        }

        let mut rng = rand::rng();
        loop {
            let key: u16 = rng.random();
            let key_str = format!("{:04x}", key);
            if entries.contains_key(&key_str) {
                continue;
            }
            entries.insert(key_str.clone(), data);
            drop(entries);
            self.save().map_err(|e| format!("failed to save memory: {e}"))?;
            return Ok(key_str);
        }
    }

    pub fn delete(&self, key: &str) -> Result<String, String> {
        let mut entries = self.entries.lock().unwrap();
        match entries.remove(key) {
            Some(_) => {
                drop(entries);
                self.save().map_err(|e| format!("failed to save memory: {e}"))?;
                Ok(format!("Deleted memory entry '{key}'."))
            }
            None => Err(format!("No memory entry with key '{key}'.")),
        }
    }

    pub fn to_markdown(&self) -> String {
        let entries = self.entries.lock().unwrap();
        if entries.is_empty() {
            "## Memory\nNo memory entries yet.".to_string()
        } else {
            let mut lines = vec!["## Memory".to_string()];
            for (key, value) in entries.iter() {
                lines.push(format!("- **{key}**: {value}"));
            }
            lines.join("\n")
        }
    }
}
