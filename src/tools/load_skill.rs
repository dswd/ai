use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::shared::ToolError;
use crate::skills::Skill;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LoadSkillArgs {
    #[schemars(description = "The name of the skill to load")]
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct LoadSkillTool {
    skills: Arc<Vec<Skill>>,
}

impl LoadSkillTool {
    pub fn new(skills: Arc<Vec<Skill>>) -> Self {
        Self { skills }
    }
}

impl PortableTool for LoadSkillTool {
    const NAME: &'static str = "load_skill";

    type Args = LoadSkillArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Load the full definition and instructions of a skill by its name, plus the absolute paths \
         of the other files bundled in the skill's folder. Available skills are listed in the \
         system prompt."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(LoadSkillArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("📖 load skill '{}'", args.name);
        let skill = self
            .skills
            .iter()
            .find(|s| s.name == args.name)
            .ok_or_else(|| {
                let available: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
                let hint = if available.is_empty() {
                    "no skills loaded".to_string()
                } else {
                    format!("available skills: {}", available.join(", "))
                };
                ToolError::Message(format!("skill '{}' not found ({hint})", args.name))
            })?;
        let body = crate::skills::load(skill).map_err(|e| {
            ToolError::Message(format!("failed to read skill '{}': {e}", skill.name))
        })?;
        let files = crate::skills::additional_files(skill);
        if files.is_empty() {
            return Ok(body);
        }
        let mut out = body;
        out.push_str("\n\nAdditional files in this skill's folder (absolute paths):\n");
        for path in files.iter().take(crate::skills::MAX_ADDITIONAL_FILES) {
            out.push_str(&format!("- {}\n", path.display()));
        }
        if files.len() > crate::skills::MAX_ADDITIONAL_FILES {
            out.push_str(&format!(
                "- … and {} more\n",
                files.len() - crate::skills::MAX_ADDITIONAL_FILES
            ));
        }
        out.push_str("\nRead any of them with the `read_file` tool.");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_load_skill_found() {
        let dir = std::env::temp_dir().join(format!("ai-skill-tool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: Test\n---\n\nInstructions body\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts").join("run.sh"), "echo hi\n").unwrap();

        let skill = Skill {
            name: "test-skill".to_string(),
            description: "Test".to_string(),
            path: dir.join("SKILL.md"),
        };
        let tool = LoadSkillTool::new(Arc::new(vec![skill]));
        let out = tool
            .call(LoadSkillArgs {
                name: "test-skill".to_string(),
            })
            .await
            .unwrap();
        assert!(out.contains("Instructions body"));
        assert!(out.contains("Additional files in this skill's folder"));
        assert!(
            out.contains("scripts/run.sh") && out.contains("/scripts/run.sh"),
            "should list the absolute bundled file path: {out}"
        );
        assert!(!out.contains("SKILL.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_load_skill_not_found() {
        let tool = LoadSkillTool::new(Arc::new(vec![]));
        let err = tool
            .call(LoadSkillArgs {
                name: "missing".to_string(),
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
