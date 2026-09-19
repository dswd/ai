use log::warn;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::tools::shared::should_skip_walk_entry;

/// Origin tag written into the front matter of AI-authored skills.
pub const AI_ORIGIN: &str = "ai";

/// Maximum body/description sizes accepted from the authoring tools.
pub const MAX_SKILL_BODY_CHARS: usize = 20_000;
pub const MAX_SKILL_DESCRIPTION_CHARS: usize = 200;

#[derive(Debug, Clone, Deserialize)]
struct SkillFrontMatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    origin: Option<String>,
}

/// Front matter serialized by the authoring tools (field order is intentional).
#[derive(Debug, Serialize)]
struct SkillFrontMatterOut<'a> {
    name: &'a str,
    description: &'a str,
    origin: &'a str,
    updated: String,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// `"ai"` for skills authored by the agent, otherwise `"user"`.
    pub origin: String,
}

impl Skill {
    pub fn ai_created(&self) -> bool {
        self.origin == AI_ORIGIN
    }
}

pub fn discover(skills_dir: &Path) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();
    find_skills_in_dir(skills_dir, &mut skills);

    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<Skill> = Vec::new();
    for skill in skills {
        if seen.insert(skill.name.clone()) {
            deduped.push(skill);
        } else {
            warn!(
                "duplicate skill name '{}' ignored ({})",
                skill.name,
                skill.path.display()
            );
        }
    }
    deduped.sort_by(|a, b| a.name.cmp(&b.name));
    deduped
}

pub fn summary(skills: &[Skill]) -> String {
    let mut lines = vec![
        "## Skills".to_string(),
        "The following skills are available. Load a skill's full instructions with the `load_skill` tool.".to_string(),
        String::new(),
    ];
    for skill in skills {
        let label = if skill.ai_created() {
            " (AI-created)"
        } else {
            ""
        };
        if skill.description.trim().is_empty() {
            lines.push(format!("- **{}**{}", skill.name, label));
        } else {
            lines.push(format!(
                "- **{}**{}: {}",
                skill.name, label, skill.description
            ));
        }
    }
    lines.join("\n")
}

pub fn load(skill: &Skill) -> std::io::Result<String> {
    std::fs::read_to_string(&skill.path)
}

/// Validate a skill name as a filesystem-safe lowercase slug.
pub fn validate_slug(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid skill name '{name}': use lowercase letters, digits and hyphens (max 64 chars)"
        ))
    }
}

/// Find a skill by name in `dir`.
pub fn find(dir: &Path, name: &str) -> Option<Skill> {
    discover(dir).into_iter().find(|s| s.name == name)
}

fn check_limits(description: &str, body: &str) -> Result<(), String> {
    if description.chars().count() > MAX_SKILL_DESCRIPTION_CHARS {
        return Err(format!(
            "description is too long (max {MAX_SKILL_DESCRIPTION_CHARS} characters)"
        ));
    }
    if body.chars().count() > MAX_SKILL_BODY_CHARS {
        return Err(format!(
            "skill body is too long (max {MAX_SKILL_BODY_CHARS} characters)"
        ));
    }
    if body.trim().is_empty() {
        return Err("skill body must not be empty".to_string());
    }
    Ok(())
}

fn render_skill_file(name: &str, description: &str, body: &str) -> Result<String, String> {
    let front = SkillFrontMatterOut {
        name,
        description,
        origin: AI_ORIGIN,
        updated: crate::util::now_iso(),
    };
    let yaml = serde_yaml_ng::to_string(&front)
        .map_err(|e| format!("failed to serialize skill metadata: {e}"))?;
    Ok(format!("---\n{yaml}---\n\n{}\n", body.trim_end()))
}

/// Create a new AI-authored skill under `dir`. Refuses names that already exist.
pub fn create(dir: &Path, name: &str, description: &str, body: &str) -> Result<PathBuf, String> {
    validate_slug(name)?;
    check_limits(description, body)?;
    if find(dir, name).is_some() {
        return Err(format!("a skill named '{name}' already exists"));
    }
    let path = dir.join(name).join("SKILL.md");
    if path.exists() {
        return Err(format!("a file already exists at {}", path.display()));
    }
    let content = render_skill_file(name, description, body)?;
    std::fs::create_dir_all(path.parent().unwrap_or(dir))
        .map_err(|e| format!("failed to create skill directory: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("failed to write skill: {e}"))?;
    Ok(path)
}

/// Update an existing AI-authored skill. Refuses skills without the AI marker.
pub fn update(
    dir: &Path,
    name: &str,
    description: Option<&str>,
    body: &str,
) -> Result<PathBuf, String> {
    validate_slug(name)?;
    let skill = find(dir, name).ok_or_else(|| format!("no skill named '{name}'"))?;
    if !skill.ai_created() {
        return Err(format!("skill '{name}' was not created by the agent"));
    }
    let description = description.unwrap_or(&skill.description);
    check_limits(description, body)?;
    let content = render_skill_file(name, description, body)?;
    std::fs::write(&skill.path, content).map_err(|e| format!("failed to write skill: {e}"))?;
    Ok(skill.path)
}

/// Delete a skill. When `require_ai` is set, only AI-authored skills may be
/// removed. Removes the skill's own directory, but only the `SKILL.md` when the
/// skill sits directly in `dir` (so the whole skills tree is never wiped).
pub fn delete(dir: &Path, name: &str, require_ai: bool) -> Result<PathBuf, String> {
    let skill = find(dir, name).ok_or_else(|| format!("no skill named '{name}'"))?;
    if require_ai && !skill.ai_created() {
        return Err(format!("skill '{name}' was not created by the agent"));
    }
    let root = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let own = skill
        .path
        .parent()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    if own.as_deref() == Some(root.as_path()) {
        std::fs::remove_file(&skill.path).map_err(|e| format!("failed to delete skill: {e}"))?;
    } else if let Some(own) = own {
        std::fs::remove_dir_all(&own).map_err(|e| format!("failed to delete skill: {e}"))?;
    }
    Ok(skill.path)
}

/// Write counters reported by the dream skill pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkillWriteStats {
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
}

/// Serialized, counter-tracking access to the skills directory for the authoring
/// tools. The mutex prevents concurrent dream reviews from clobbering each other.
#[derive(Debug)]
pub struct SkillStore {
    dir: PathBuf,
    lock: Mutex<()>,
    stats: Mutex<SkillWriteStats>,
}

impl SkillStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            lock: Mutex::new(()),
            stats: Mutex::new(SkillWriteStats::default()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn stats(&self) -> SkillWriteStats {
        *self.stats.lock().unwrap()
    }

    pub fn create(&self, name: &str, description: &str, body: &str) -> Result<PathBuf, String> {
        let _guard = self.lock.lock().unwrap();
        let path = create(&self.dir, name, description, body)?;
        self.stats.lock().unwrap().created += 1;
        log::info!("created skill '{name}' at {}", path.display());
        Ok(path)
    }

    pub fn update(
        &self,
        name: &str,
        description: Option<&str>,
        body: &str,
    ) -> Result<PathBuf, String> {
        let _guard = self.lock.lock().unwrap();
        let path = update(&self.dir, name, description, body)?;
        self.stats.lock().unwrap().updated += 1;
        log::info!("updated skill '{name}' at {}", path.display());
        Ok(path)
    }

    pub fn delete(&self, name: &str) -> Result<PathBuf, String> {
        let _guard = self.lock.lock().unwrap();
        let path = delete(&self.dir, name, true)?;
        self.stats.lock().unwrap().deleted += 1;
        log::info!("deleted skill '{name}' at {}", path.display());
        Ok(path)
    }
}

/// Cap on the number of bundled files reported by `load_skill`.
pub const MAX_ADDITIONAL_FILES: usize = 200;

/// Other files in the skill's folder, as absolute paths (the skill's own
/// `SKILL.md` excluded). Hidden entries and build directories are skipped.
pub fn additional_files(skill: &Skill) -> Vec<PathBuf> {
    let Some(dir) = skill.path.parent() else {
        return Vec::new();
    };
    let root = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let skill_file = std::fs::canonicalize(&skill.path).ok();
    let mut out = Vec::new();
    collect_files(&root, skill_file.as_deref(), &mut out);
    out.sort();
    out
}

fn collect_files(dir: &Path, exclude: Option<&Path>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_walk_entry(&name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, exclude, out);
        } else if path.is_file() && exclude != std::fs::canonicalize(&path).ok().as_deref() {
            out.push(path);
        }
    }
}

fn find_skills_in_dir(dir: &Path, out: &mut Vec<Skill>) {
    if !dir.exists() {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(e) => e.flatten().collect(),
        Err(e) => {
            warn!("cannot read skills dir {}: {e}", dir.display());
            return;
        }
    };
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if should_skip_walk_entry(&name) {
            continue;
        }

        if name == "SKILL.md" {
            if path.is_file()
                && let Some(skill) = parse_skill_file(&path)
            {
                out.push(skill);
            }
        } else if path.is_dir() {
            find_skills_in_dir(&path, out);
        }
    }
}

fn parse_skill_file(path: &Path) -> Option<Skill> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warn!("cannot read skill file {}: {e}", path.display());
            return None;
        }
    };

    let (front_name, front_desc, front_origin) = match parse_front_matter(&content) {
        Some(fm) => (fm.name, fm.description, fm.origin),
        None => (None, None, None),
    };

    let name = front_name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            let fallback = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                });
            warn!(
                "skill {} has no name in front matter; using '{}'",
                path.display(),
                fallback
            );
            fallback
        });

    Some(Skill {
        name,
        description: front_desc.unwrap_or_default(),
        path: path.to_path_buf(),
        origin: front_origin
            .filter(|o| !o.trim().is_empty())
            .unwrap_or_else(|| "user".to_string()),
    })
}

fn parse_front_matter(content: &str) -> Option<SkillFrontMatter> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let content = if content.contains("\r\n") {
        content.replace("\r\n", "\n")
    } else {
        content.to_string()
    };
    if !content.starts_with("---\n") {
        return None;
    }

    let rest = &content[4..];
    let lines: Vec<&str> = rest.split('\n').collect();
    let close_idx = lines.iter().position(|l| *l == "---")?;
    let block = lines[..close_idx].join("\n");

    match serde_yaml_ng::from_str::<SkillFrontMatter>(&block) {
        Ok(fm) => Some(fm),
        Err(e) => {
            warn!("invalid front matter: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ai-skills-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_parse_front_matter_valid() {
        let content = "---\nname: my-skill\ndescription: Does things\n---\n\nBody here\n";
        let fm = parse_front_matter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("my-skill"));
        assert_eq!(fm.description.as_deref(), Some("Does things"));
    }

    #[test]
    fn test_parse_front_matter_missing_description() {
        let content = "---\nname: my-skill\n---\n\nBody here\n";
        let fm = parse_front_matter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("my-skill"));
        assert_eq!(fm.description, None);
    }

    #[test]
    fn test_parse_front_matter_none() {
        assert!(parse_front_matter("no front matter here").is_none());
        assert!(parse_front_matter("").is_none());
    }

    #[test]
    fn test_parse_front_matter_crlf() {
        let content =
            "---\r\nname: win-skill\r\ndescription: Windows file\r\n---\r\n\r\nBody here\r\n";
        let fm = parse_front_matter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("win-skill"));
        assert_eq!(fm.description.as_deref(), Some("Windows file"));
    }

    #[test]
    fn test_parse_front_matter_ignores_rule_line() {
        let content = "---\nname: my-skill\ndescription: |\n  Some text\n  --- note is not a delimiter\n  more text\n---\n\nBody here\n";
        let fm = parse_front_matter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("my-skill"));
        assert!(fm.description.unwrap().contains("more text"));
    }

    #[test]
    fn test_summary_empty_description() {
        let dir = temp_dir("summary-empty");
        std::fs::create_dir_all(dir.join("no-desc")).unwrap();
        std::fs::create_dir_all(dir.join("with-desc")).unwrap();
        std::fs::write(
            dir.join("no-desc").join("SKILL.md"),
            "---\nname: no-desc\n---\nBody",
        )
        .unwrap();
        std::fs::write(
            dir.join("with-desc").join("SKILL.md"),
            "---\nname: with-desc\ndescription: Has one\n---\nBody",
        )
        .unwrap();
        let skills = discover(&dir);
        let s = summary(&skills);
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines.contains(&"- **no-desc**"));
        assert!(!s.contains("- **no-desc**:"));
        assert!(lines.contains(&"- **with-desc**: Has one"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_from_dir() {
        let dir = temp_dir("discover");
        std::fs::create_dir_all(dir.join("foo")).unwrap();
        std::fs::create_dir_all(dir.join("bar")).unwrap();
        std::fs::write(
            dir.join("foo").join("SKILL.md"),
            "---\nname: foo-skill\ndescription: Foo skill\n---\nBody",
        )
        .unwrap();
        std::fs::write(
            dir.join("bar").join("SKILL.md"),
            "---\nname: bar-skill\n---\nBody",
        )
        .unwrap();

        let skills = discover(&dir);
        assert_eq!(skills.len(), 2);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo-skill"));
        assert!(names.contains(&"bar-skill"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dedup_by_name() {
        let dir = temp_dir("dedup");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(
            dir.join("a").join("SKILL.md"),
            "---\nname: same\ndescription: first\n---\nBody",
        )
        .unwrap();
        std::fs::write(
            dir.join("b").join("SKILL.md"),
            "---\nname: same\ndescription: second\n---\nBody",
        )
        .unwrap();

        let skills = discover(&dir);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_summary_format() {
        let dir = temp_dir("summary");
        std::fs::create_dir_all(dir.join("foo")).unwrap();
        std::fs::write(
            dir.join("foo").join("SKILL.md"),
            "---\nname: foo\ndescription: Bar baz\n---\nBody",
        )
        .unwrap();
        let skills = discover(&dir);
        let s = summary(&skills);
        assert!(s.contains("## Skills"));
        assert!(s.contains("**foo**: Bar baz"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_returns_full_content() {
        let dir = temp_dir("load");
        std::fs::create_dir_all(dir.join("foo")).unwrap();
        let content = "---\nname: foo\n---\n\nBody instructions\n";
        std::fs::write(dir.join("foo").join("SKILL.md"), content).unwrap();
        let skills = discover(&dir);
        assert_eq!(load(&skills[0]).unwrap(), content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_additional_files_absolute_sorted_and_filtered() {
        let dir = temp_dir("files");
        let skill_dir = dir.join("foo");
        std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        std::fs::create_dir_all(skill_dir.join("reference")).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: foo\ndescription: Foo\n---\nBody",
        )
        .unwrap();
        std::fs::write(skill_dir.join("scripts").join("run.sh"), "echo hi\n").unwrap();
        std::fs::write(skill_dir.join("reference").join("notes.md"), "notes\n").unwrap();
        std::fs::write(skill_dir.join(".hidden"), "secret\n").unwrap();
        std::fs::create_dir_all(skill_dir.join("node_modules")).unwrap();
        std::fs::write(skill_dir.join("node_modules").join("junk.js"), "junk\n").unwrap();

        let skills = discover(&dir);
        let files = additional_files(&skills[0]);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["notes.md", "run.sh"]);
        assert!(
            files.iter().all(|p| p.is_absolute()),
            "paths must be absolute: {files:?}"
        );
        let mut sorted = files.clone();
        sorted.sort();
        assert_eq!(files, sorted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_additional_files_empty_when_only_skill() {
        let dir = temp_dir("nofiles");
        let skill_dir = dir.join("foo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: foo\n---\nBody").unwrap();
        let skills = discover(&dir);
        assert!(additional_files(&skills[0]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_origin_parsed_and_marked_in_summary() {
        let dir = temp_dir("origin");
        std::fs::create_dir_all(dir.join("hand")).unwrap();
        std::fs::create_dir_all(dir.join("auto")).unwrap();
        std::fs::write(
            dir.join("hand").join("SKILL.md"),
            "---\nname: hand\ndescription: By hand\n---\nBody",
        )
        .unwrap();
        std::fs::write(
            dir.join("auto").join("SKILL.md"),
            "---\nname: auto\ndescription: By agent\norigin: ai\n---\nBody",
        )
        .unwrap();
        let skills = discover(&dir);
        let auto = skills.iter().find(|s| s.name == "auto").unwrap();
        assert!(auto.ai_created());
        let hand = skills.iter().find(|s| s.name == "hand").unwrap();
        assert!(!hand.ai_created());
        let text = summary(&skills);
        assert!(text.contains("**auto** (AI-created): By agent"));
        assert!(text.contains("**hand**: By hand"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_update_delete_lifecycle() {
        let dir = temp_dir("lifecycle");
        let path = create(&dir, "my-skill", "Does things", "Step one\nStep two").unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), "SKILL.md");
        let skill = find(&dir, "my-skill").unwrap();
        assert!(skill.ai_created());
        assert_eq!(skill.description, "Does things");

        // create refuses a duplicate name.
        assert!(create(&dir, "my-skill", "Again", "Body").is_err());

        // update only touches AI skills.
        std::fs::create_dir_all(dir.join("hand")).unwrap();
        std::fs::write(
            dir.join("hand").join("SKILL.md"),
            "---\nname: hand\n---\nBody",
        )
        .unwrap();
        assert!(update(&dir, "hand", None, "new").is_err());

        update(&dir, "my-skill", Some("Updated"), "New body").unwrap();
        let skill = find(&dir, "my-skill").unwrap();
        assert_eq!(skill.description, "Updated");

        delete(&dir, "my-skill", true).unwrap();
        assert!(find(&dir, "my-skill").is_none());
        // user skill survives an AI-scoped delete.
        assert!(delete(&dir, "hand", true).is_err());
        // and the whole tree is intact.
        assert!(dir.join("hand").join("SKILL.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_root_level_skill_keeps_tree() {
        let dir = temp_dir("rootdelete");
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: rootish\ndescription: Root\n---\nBody",
        )
        .unwrap();
        // A sibling skill directory must survive.
        std::fs::create_dir_all(dir.join("other")).unwrap();
        std::fs::write(
            dir.join("other").join("SKILL.md"),
            "---\nname: other\n---\nB",
        )
        .unwrap();
        delete(&dir, "rootish", false).unwrap();
        assert!(!dir.join("SKILL.md").exists());
        assert!(dir.join("other").join("SKILL.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_slug() {
        assert!(validate_slug("good-name-1").is_ok());
        for bad in ["", "UPPER", "../escape", "with space", "-leading", "a/b"] {
            assert!(validate_slug(bad).is_err(), "{bad}");
        }
    }
}
