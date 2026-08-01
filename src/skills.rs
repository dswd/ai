use log::warn;
use serde::Deserialize;
use std::path::{Path, PathBuf};

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

pub fn discover(skill_args: &[String], skills_dir: &Path) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();

    for arg in skill_args {
        let path = PathBuf::from(arg);
        if path.is_file() {
            if let Some(skill) = parse_skill_file(&path) {
                skills.push(skill);
            }
        } else if path.is_dir() {
            find_skills_in_dir(&path, &mut skills);
        } else {
            warn!("skill path not found: {arg}");
        }
    }

    find_skills_in_dir(skills_dir, &mut skills);

    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<Skill> = Vec::new();
    for skill in skills {
        if seen.insert(skill.name.clone()) {
            deduped.push(skill);
        } else {
            warn!("duplicate skill name '{}' ignored ({})", skill.name, skill.path.display());
        }
    }
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

pub fn load(skill: &Skill) -> Result<String, String> {
    std::fs::read_to_string(&skill.path)
        .map_err(|e| format!("failed to read skill '{}': {e}", skill.name))
}

fn find_skills_in_dir(dir: &Path, out: &mut Vec<Skill>) {
    if !dir.exists() {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("cannot read skills dir {}: {e}", dir.display());
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

        if name.starts_with('.') {
            continue;
        }
        if name == "node_modules" || name == "target" || name == ".git" {
            continue;
        }

        if path.is_dir() {
            find_skills_in_dir(&path, out);
        } else if path.is_file()
            && name == "SKILL.md"
            && let Some(skill) = parse_skill_file(&path)
        {
            out.push(skill);
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
    let fallback_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    Some(Skill {
        name: front_name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| {
                if fallback_name.is_empty() {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    warn!(
                        "skill {} has no name in front matter; using file stem '{}'",
                        path.display(),
                        stem
                    );
                    stem
                } else {
                    fallback_name
                }
            }),
        description: front_desc.unwrap_or_default(),
        path: path.to_path_buf(),
    })
}

fn parse_front_matter(content: &str) -> Option<SkillFrontMatter> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let content = content.replace("\r\n", "\n");
    if !content.starts_with("---\n") {
        return None;
    }

    let rest = &content[4..];
    let lines: Vec<&str> = rest.split('\n').collect();
    let close_idx = lines.iter().position(|l| *l == "---")?;
    let block = lines[..close_idx].join("\n");

    match serde_yaml::from_str::<SkillFrontMatter>(&block) {
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
        let dir = std::env::temp_dir().join(format!("ai-skills-test-{name}-{}", std::process::id()));
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
        let content = "---\r\nname: win-skill\r\ndescription: Windows file\r\n---\r\n\r\nBody here\r\n";
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
        std::fs::write(dir.join("no-desc").join("SKILL.md"), "---\nname: no-desc\n---\nBody").unwrap();
        std::fs::write(
            dir.join("with-desc").join("SKILL.md"),
            "---\nname: with-desc\ndescription: Has one\n---\nBody",
        )
        .unwrap();
        let skills = discover(&[], &dir);
        let s = summary(&skills);
        assert!(s.contains("- **no-desc**\n"));
        assert!(!s.contains("- **no-desc**:"));
        assert!(s.contains("- **with-desc**: Has one"));
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
        std::fs::write(dir.join("bar").join("SKILL.md"), "---\nname: bar-skill\n---\nBody").unwrap();

        let skills = discover(&[], &dir);
        assert_eq!(skills.len(), 2);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo-skill"));
        assert!(names.contains(&"bar-skill"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_direct_file_arg() {
        let dir = temp_dir("filearg");
        let file = dir.join("SKILL.md");
        std::fs::write(
            &file,
            "---\nname: direct\ndescription: From direct file\n---\nBody",
        )
        .unwrap();

        let skills = discover(&[file.to_string_lossy().to_string()], &dir.join("nonexistent"));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "direct");
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

        let skills = discover(&[], &dir);
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
        let skills = discover(&[], &dir);
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
        let skills = discover(&[], &dir);
        assert_eq!(load(&skills[0]).unwrap(), content);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
