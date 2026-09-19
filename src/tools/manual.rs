use std::sync::OnceLock;

use clap::CommandFactory;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;

const README: &str = include_str!("../../README.md");
const CONFIG_EXAMPLE: &str = include_str!("../../config.example.yaml");
const MANUAL: &str = include_str!("../../docs/manual.md");

/// Upper bound on a single manual response.
const MAX_MANUAL_CHARS: usize = 24_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManualArgs {
    #[schemars(
        description = "Optional topic, e.g. \"flags\", \"configuration\", \"memory\", \"skills\", \
                       \"container\", or a command name. Omit to list all topics."
    )]
    #[serde(default)]
    pub topic: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ManualTool;

impl Default for ManualTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualTool {
    pub fn new() -> Self {
        Self
    }
}

impl PortableTool for ManualTool {
    const NAME: &'static str = "manual";

    type Args = ManualArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Look up the `ai` documentation: commands, flags, configuration keys, and how features \
         (policy, memory, skills, containers, web search) work. Use this to answer questions \
         about `ai` itself instead of guessing. Call with no topic for the list of topics."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ManualArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let entries = all_entries();
        match args.topic.as_deref().map(str::trim) {
            Some(topic) if !topic.is_empty() => Ok(select(entries, topic)),
            _ => Ok(toc(entries)),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    title: String,
    body: String,
}

/// Parse `##` sections from markdown, tracking fenced code blocks so a `## `
/// inside a fence is treated as content.
fn section_entries(md: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    let mut in_fence = false;
    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
        }
        let heading = if !in_fence {
            trimmed.strip_prefix("## ")
        } else {
            None
        };
        if let Some(title) = heading {
            if let Some((title, body)) = current.take() {
                entries.push(Entry {
                    title,
                    body: body.join("\n").trim_end().to_string(),
                });
            }
            current = Some((title.trim().to_string(), vec![line]));
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((title, body)) = current {
        entries.push(Entry {
            title,
            body: body.join("\n").trim_end().to_string(),
        });
    }
    entries
}

/// Manual sections take precedence over README sections with the same title.
fn merge_entries(manual: Vec<Entry>, readme: Vec<Entry>) -> Vec<Entry> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for entry in manual.into_iter().chain(readme) {
        if seen.insert(entry.title.to_lowercase()) {
            out.push(entry);
        }
    }
    out
}

fn clap_entries() -> Vec<Entry> {
    let mut root = crate::cli::Cli::command();
    let mut entries = vec![Entry {
        title: "Command-line flags".to_string(),
        body: format!("# Command-line flags\n\n{}", root.render_long_help()),
    }];
    for sub in root.get_subcommands_mut() {
        let name = sub.get_name().to_string();
        entries.push(Entry {
            title: name.clone(),
            body: format!("# ai {name}\n\n{}", sub.render_long_help()),
        });
    }
    entries
}

fn all_entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let prose = merge_entries(section_entries(MANUAL), section_entries(README));
        let mut entries = prose;
        entries.extend(clap_entries());
        entries.push(Entry {
            title: "Configuration reference".to_string(),
            body: format!(
                "# Configuration reference (config.example.yaml)\n\n```yaml\n{}\n```",
                CONFIG_EXAMPLE.trim_end()
            ),
        });
        entries
    })
}

fn blurb(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```"))
        .map(|l| l.trim_start_matches(['*', '-', ' ']).to_string())
        .unwrap_or_default()
        .chars()
        .take(90)
        .collect()
}

fn toc(entries: &[Entry]) -> String {
    let mut lines = vec![
        "# ai manual — topics".to_string(),
        "Ask `manual` with a topic to get that section (e.g. `manual(\"flags\")`, \
         `manual(\"configuration\")`, `manual(\"memory\")`)."
            .to_string(),
    ];
    for entry in entries {
        lines.push(format!("- {}: {}", entry.title, blurb(&entry.body)));
    }
    lines.join("\n")
}

fn select(entries: &[Entry], topic: &str) -> String {
    let needle = topic.to_lowercase();
    let title_matches = |exact: bool| -> Vec<&Entry> {
        entries
            .iter()
            .filter(|e| {
                let title = e.title.to_lowercase();
                if exact {
                    title == needle
                } else {
                    title.contains(&needle)
                }
            })
            .collect()
    };
    let mut matches = title_matches(true);
    if matches.is_empty() {
        matches = title_matches(false);
    }
    if matches.is_empty() {
        return format!(
            "No section matched \"{topic}\". Try one of these:\n\n{}",
            toc(entries)
        );
    }
    let mut out = matches
        .iter()
        .map(|e| e.body.as_str())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    if out.chars().count() > MAX_MANUAL_CHARS {
        out = out.chars().take(MAX_MANUAL_CHARS).collect();
        out.push_str("\n\n[truncated; ask for a narrower topic]");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_section_entries_ignores_non_level_two_and_fences() {
        let md = "# Title\n\nintro\n\n## First\nbody one\n\n### Sub\nnot a section\n\n```\n## fake\n```\n\n## Second\nbody two\n";
        let entries = section_entries(md);
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["First", "Second"]);
        assert!(entries[0].body.contains("body one"));
        assert!(entries[0].body.contains("### Sub"));
        assert!(entries[0].body.contains("## fake"));
    }

    #[test]
    fn test_merge_prefers_manual() {
        let manual = vec![Entry {
            title: "Memory".into(),
            body: "manual memory".into(),
        }];
        let readme = vec![
            Entry {
                title: "memory".into(),
                body: "readme memory".into(),
            },
            Entry {
                title: "Only in readme".into(),
                body: "extra".into(),
            },
        ];
        let merged = merge_entries(manual, readme);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].body, "manual memory");
        assert_eq!(merged[1].title, "Only in readme");
    }

    #[test]
    fn test_select_substring_and_fallback() {
        let entries = vec![
            Entry {
                title: "Memory".into(),
                body: "memory body".into(),
            },
            Entry {
                title: "Memory maintenance".into(),
                body: "dream body".into(),
            },
        ];
        let out = select(&entries, "memory");
        assert!(out.contains("memory body"));
        // An exact title match wins over the substring match.
        assert!(!out.contains("dream body"));
        let substring = select(&entries, "maintenance");
        assert!(substring.contains("dream body"));
        let miss = select(&entries, "nonexistent");
        assert!(miss.contains("No section matched"));
        assert!(miss.contains("topics"));
    }

    #[tokio::test]
    async fn test_manual_flags_topic() {
        let out = ManualTool::new()
            .call(ManualArgs {
                topic: Some("flags".into()),
            })
            .await
            .unwrap();
        assert!(out.contains("--yolo"), "{out}");
    }

    #[tokio::test]
    async fn test_manual_configuration_topic() {
        let out = ManualTool::new()
            .call(ManualArgs {
                topic: Some("configuration".into()),
            })
            .await
            .unwrap();
        assert!(out.contains("memory_max_distance"), "{out}");
    }

    #[tokio::test]
    async fn test_manual_subcommand_topic() {
        let out = ManualTool::new()
            .call(ManualArgs {
                topic: Some("run".into()),
            })
            .await
            .unwrap();
        assert!(
            out.to_lowercase().contains("run the agent") || out.contains("Run"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn test_manual_no_topic_returns_topics() {
        let out = ManualTool::new()
            .call(ManualArgs { topic: None })
            .await
            .unwrap();
        assert!(out.contains("topics"));
        assert!(out.contains("memory"));
    }
}
