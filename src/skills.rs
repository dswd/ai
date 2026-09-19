use log::warn;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::tools::shared::should_skip_walk_entry;

#[derive(Debug, Clone, Deserialize)]
struct SkillFrontMatter {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
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
        if skill.description.trim().is_empty() {
            lines.push(format!("- **{}**", skill.name));
        } else {
            lines.push(format!("- **{}**: {}", skill.name, skill.description));
        }
    }
    lines.join("\n")
}

pub fn load(skill: &Skill) -> std::io::Result<String> {
    std::fs::read_to_string(&skill.path)
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

    let (front_name, front_desc) = match parse_front_matter(&content) {
        Some(fm) => (fm.name, fm.description),
        None => (None, None),
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
}
