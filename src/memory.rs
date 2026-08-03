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
            let content = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("reading memory file {}: {e}", path.display()))?;
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
            self.save()
                .map_err(|e| format!("failed to save memory: {e}"))?;
            return Ok(key_str);
        }
    }

    pub fn delete(&self, key: &str) -> Result<String, String> {
        let mut entries = self.entries.lock().unwrap();
        match entries.remove(key) {
            Some(_) => {
                drop(entries);
                self.save()
                    .map_err(|e| format!("failed to save memory: {e}"))?;
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

    #[test]
    fn test_empty_memory() {
        let (dir, mem) = temp_memory("empty");
        assert_eq!(mem.to_markdown(), "## Memory\nNo memory entries yet.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_add_and_to_markdown() {
        let (dir, mem) = temp_memory("add");
        let key = mem.add("some data".to_string()).unwrap();
        let md = mem.to_markdown();
        assert!(md.contains(&format!("- **{key}**: some data")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete() {
        let (dir, mem) = temp_memory("delete");
        let key = mem.add("data".to_string()).unwrap();
        assert!(mem.delete(&key).is_ok());
        assert!(mem.delete(&key).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_reload_persists() {
        let (dir, mem) = temp_memory("reload");
        let path = dir.join("memory.json");
        let key = mem.add("persisted".to_string()).unwrap();
        drop(mem);
        let reloaded = Memory::load(&path).unwrap();
        assert!(
            reloaded
                .to_markdown()
                .contains(&format!("- **{key}**: persisted"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
