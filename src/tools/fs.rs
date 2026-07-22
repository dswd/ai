use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{finalize_output, resolve_path, ToolError, MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES};
use crate::policy::{Action, Policy};

// ---------------------------------------------------------------------------
// Args structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListDirArgs {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReplaceInFileArgs {
    pub path: String,
    pub old_str: String,
    pub new_str: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeleteFileArgs {
    pub path: String,
    pub recursive: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateDirectoryArgs {
    pub path: String,
}

// ---------------------------------------------------------------------------
// ReadFileTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReadFileTool {
    policy: Policy,
}

impl ReadFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for ReadFileTool {
    const NAME: &'static str = "read_file";

    type Args = ReadFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Read a file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ReadFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📄  read {}{RESET}", args.path);
        let canonical = resolve_path(&args.path, &self.policy, &Action::Read)?;
        let content = std::fs::read_to_string(&canonical)
            .map_err(|e| ToolError::Message(format!("cannot read: {e}")))?;
        finalize_output(&content, args.offset, args.limit, &args.path)
    }
}

// ---------------------------------------------------------------------------
// WriteFileTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WriteFileTool {
    policy: Policy,
}

impl WriteFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for WriteFileTool {
    const NAME: &'static str = "write_file";

    type Args = WriteFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Write content to a file (creates/overwrites)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WriteFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}✏️  write {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let canonical = if path.exists() {
            resolve_path(&args.path, &self.policy, &Action::Write)?
        } else {
            let parent = path.parent().ok_or_else(|| {
                ToolError::Message("cannot determine parent directory".to_string())
            })?;
            let canonical_parent = parent
                .canonicalize()
                .map_err(|e| ToolError::Message(format!("cannot resolve parent: {e}")))?;
            if !self
                .policy
                .is_allowed(&Action::Write, &canonical_parent.to_string_lossy())
            {
                return Err(ToolError::Message(format!("write access denied for: {}", args.path)));
            }
            canonical_parent.join(path.file_name().unwrap_or_default())
        };

        if let Some(parent) = canonical.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Message(format!("cannot create dirs: {e}")))?;
        }

        std::fs::write(&canonical, &args.content)
            .map_err(|e| ToolError::Message(format!("cannot write: {e}")))?;

        let msg = format!("Wrote to {}", args.path);
        info!("  → {msg}");
        Ok(msg)
    }
}

// ---------------------------------------------------------------------------
// ListDirTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ListDirTool {
    policy: Policy,
}

impl ListDirTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for ListDirTool {
    const NAME: &'static str = "list_dir";

    type Args = ListDirArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "List files and directories".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ListDirArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📂  list {}{RESET}", args.path);
        let canonical = resolve_path(&args.path, &self.policy, &Action::Read)?;

        let entries: Vec<String> = std::fs::read_dir(&canonical)
            .map_err(|e| ToolError::Message(format!("cannot read dir: {e}")))?
            .filter_map(|entry| {
                entry.ok().map(|e| {
                    let ft = match e.file_type() {
                        Ok(t) if t.is_dir() => 'd',
                        Ok(t) if t.is_symlink() => 'l',
                        _ => 'f',
                    };
                    format!("{ft} {}", e.file_name().to_string_lossy())
                })
            })
            .collect();

        let result = entries.join("\n");
        finalize_output(&result, args.offset, args.limit, &args.path)
    }
}

// ---------------------------------------------------------------------------
// ReplaceInFileTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReplaceInFileTool {
    policy: Policy,
}

impl ReplaceInFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for ReplaceInFileTool {
    const NAME: &'static str = "replace_in_file";

    type Args = ReplaceInFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Replace a string in a file (must match exactly once)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ReplaceInFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📝  edit {}{RESET}", args.path);
        let canonical = resolve_path(&args.path, &self.policy, &Action::Write)?;

        let content = std::fs::read_to_string(&canonical)
            .map_err(|e| ToolError::Message(format!("cannot read: {e}")))?;

        if args.old_str.is_empty() {
            return Err(ToolError::Message("old_str must not be empty".to_string()));
        }

        let count = content.matches(&args.old_str).count();
        if count == 0 {
            return Err(ToolError::Message(format!(
                "old_str not found in: {}",
                args.path
            )));
        }
        if count > 1 {
            return Err(ToolError::Message(format!(
                "old_str found {count}x in {} (must be unique)",
                args.path
            )));
        }

        let new_content = content.replacen(&args.old_str, &args.new_str, 1);
        std::fs::write(&canonical, &new_content)
            .map_err(|e| ToolError::Message(format!("cannot write: {e}")))?;

        let msg = format!("Replaced 1 occurrence in {}", args.path);
        info!("  → {msg}");
        Ok(msg)
    }
}

// ---------------------------------------------------------------------------
// DeleteFileTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DeleteFileTool {
    policy: Policy,
}

impl DeleteFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for DeleteFileTool {
    const NAME: &'static str = "delete_file";

    type Args = DeleteFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Delete a file or directory".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(DeleteFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}✂️  delete {}{RESET}", args.path);
        let canonical = resolve_path(&args.path, &self.policy, &Action::Write)?;

        if canonical.is_dir() {
            if !args.recursive.unwrap_or(false) {
                return Err(ToolError::Message(format!(
                    "{} is a dir; set recursive=true to delete",
                    args.path
                )));
            }
            std::fs::remove_dir_all(&canonical)
                .map_err(|e| ToolError::Message(format!("cannot delete dir: {e}")))?;
            Ok(format!("Deleted dir: {}", args.path))
        } else {
            std::fs::remove_file(&canonical)
                .map_err(|e| ToolError::Message(format!("cannot delete file: {e}")))?;
            Ok(format!("Deleted file: {}", args.path))
        }
    }
}

// ---------------------------------------------------------------------------
// CreateDirectoryTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreateDirectoryTool {
    policy: Policy,
}

impl CreateDirectoryTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for CreateDirectoryTool {
    const NAME: &'static str = "create_directory";

    type Args = CreateDirectoryArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Create a directory (mkdir -p)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(CreateDirectoryArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📁  mkdir {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);

        let canonical = if path.exists() {
            resolve_path(&args.path, &self.policy, &Action::Write)?
        } else {
            let parent = path.parent().ok_or_else(|| {
                ToolError::Message("cannot determine parent dir".to_string())
            })?;
            if parent.exists() {
                let cp = parent
                    .canonicalize()
                    .map_err(|e| ToolError::Message(format!("cannot resolve parent: {e}")))?;
                if !self
                    .policy
                    .is_allowed(&Action::Write, &cp.to_string_lossy())
                {
                    return Err(ToolError::Message(format!(
                        "write access denied for: {}",
                        args.path
                    )));
                }
                cp.join(path.file_name().unwrap_or_default())
            } else {
                path.clone()
            }
        };

        std::fs::create_dir_all(&canonical)
            .map_err(|e| ToolError::Message(format!("cannot create dir: {e}")))?;

        Ok(format!("Created dir: {}", args.path))
    }
}
