use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::util::{bar_line, bar_title};

use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, truncate, process_output};
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    #[schemars(description = "Path to the file to read")]
    pub path: String,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolExecError {
    #[error("{0}")]
    Message(String),
}

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
    type Error = ToolExecError;

    fn description(&self) -> String {
        "Read the contents of a file at the given path".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ReadFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📄  read file {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let canonical = path
            .canonicalize()
            .map_err(|e| ToolExecError::Message(format!("cannot resolve path: {e}")))?;
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Read, &canonical_str) {
            return Err(ToolExecError::Message(format!(
                "read access denied for: {}",
                args.path
            )));
        }

        let content = std::fs::read_to_string(&canonical)
            .map_err(|e| ToolExecError::Message(format!("cannot read file: {e}")))?;
        let truncated = truncate(&content, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title(&args.path),
            bar_line()
        );
        Ok(process_output(&content, args.offset, args.limit)
            .map_err(|e| ToolExecError::Message(e))?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    #[schemars(description = "Path to the file to write")]
    pub path: String,
    #[schemars(description = "Content to write to the file")]
    pub content: String,
}

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
    type Error = ToolExecError;

    fn description(&self) -> String {
        "Write content to a file at the given path. Creates or overwrites the file.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WriteFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}✏️  write file {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let canonical = if path.exists() {
            path.canonicalize()
                .map_err(|e| ToolExecError::Message(format!("cannot resolve path: {e}")))?
        } else {
            if let Some(parent) = path.parent() {
                parent
                    .canonicalize()
                    .map_err(|e| ToolExecError::Message(format!("cannot resolve parent: {e}")))?
                    .join(path.file_name().unwrap_or_default())
            } else {
                path.clone()
            }
        };
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Write, &canonical_str) {
            return Err(ToolExecError::Message(format!(
                "write access denied for: {}",
                args.path
            )));
        }

        if let Some(parent) = canonical.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolExecError::Message(format!("cannot create parent dirs: {e}")))?;
        }

        std::fs::write(&canonical, &args.content)
            .map_err(|e| ToolExecError::Message(format!("cannot write file: {e}")))?;

        let result = format!("Successfully wrote to {}", args.path);
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListDirArgs {
    #[schemars(description = "Path to the directory to list")]
    pub path: String,
    #[schemars(description = "Line number to start listing from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of entries to return")]
    pub limit: Option<usize>,
}

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
    type Error = ToolExecError;

    fn description(&self) -> String {
        "List files and directories at the given path".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ListDirArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📂  list dir {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let canonical = path
            .canonicalize()
            .map_err(|e| ToolExecError::Message(format!("cannot resolve path: {e}")))?;
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Read, &canonical_str) {
            return Err(ToolExecError::Message(format!(
                "read access denied for: {}",
                args.path
            )));
        }

        let entries: Vec<String> = std::fs::read_dir(&canonical)
            .map_err(|e| ToolExecError::Message(format!("cannot read dir: {e}")))?
            .filter_map(|entry| {
                entry.ok().map(|e| {
                    let ftype = e.file_type().ok().map_or('?', |ft| {
                        if ft.is_dir() {
                            'd'
                        } else if ft.is_symlink() {
                            'l'
                        } else {
                            'f'
                        }
                    });
                    format!("{ftype} {}", e.file_name().to_string_lossy())
                })
            })
            .collect();

        let result = entries.join("\n");
        let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title(&args.path),
            bar_line()
        );
        Ok(process_output(&result, args.offset, args.limit)
            .map_err(|e| ToolExecError::Message(e))?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReplaceInFileArgs {
    #[schemars(description = "Path to the file to modify")]
    pub path: String,
    #[schemars(description = "Exact string to replace")]
    pub old_str: String,
    #[schemars(description = "Replacement string")]
    pub new_str: String,
}

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
    type Error = ToolExecError;

    fn description(&self) -> String {
        "Replace a specific string in a file with a new string. \
         The old_str must match exactly once in the file. \
         Use this for surgical edits instead of overwriting the entire file."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ReplaceInFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📝  edit file {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let canonical = path
            .canonicalize()
            .map_err(|e| ToolExecError::Message(format!("cannot resolve path: {e}")))?;
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Write, &canonical_str) {
            return Err(ToolExecError::Message(format!(
                "write access denied for: {}",
                args.path
            )));
        }

        let content = std::fs::read_to_string(&canonical)
            .map_err(|e| ToolExecError::Message(format!("cannot read file: {e}")))?;

        if args.old_str.is_empty() {
            return Err(ToolExecError::Message(
                "old_str must not be empty".to_string(),
            ));
        }

        let count = content.matches(&args.old_str).count();
        if count == 0 {
            return Err(ToolExecError::Message(format!(
                "old_str not found in file: {}",
                args.path
            )));
        }
        if count > 1 {
            return Err(ToolExecError::Message(format!(
                "old_str found {} times in file (must be unique): {}",
                count, args.path
            )));
        }

        let new_content = content.replacen(&args.old_str, &args.new_str, 1);

        std::fs::write(&canonical, &new_content)
            .map_err(|e| ToolExecError::Message(format!("cannot write file: {e}")))?;

        let result = format!("Successfully replaced in {} (1 occurrence)", args.path);
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeleteFileArgs {
    #[schemars(description = "Path to the file or directory to delete")]
    pub path: String,
    #[schemars(description = "If deleting a directory, remove recursively")]
    pub recursive: Option<bool>,
}

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
    type Error = ToolExecError;

    fn description(&self) -> String {
        "Delete a file or directory at the given path".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(DeleteFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}✂️  delete file {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let canonical = path
            .canonicalize()
            .map_err(|e| ToolExecError::Message(format!("cannot resolve path: {e}")))?;
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Write, &canonical_str) {
            return Err(ToolExecError::Message(format!(
                "write access denied for: {}",
                args.path
            )));
        }

        let result = if canonical.is_dir() {
            if args.recursive.unwrap_or(false) {
                std::fs::remove_dir_all(&canonical)
                    .map_err(|e| ToolExecError::Message(format!("cannot delete directory: {e}")))?;
                format!("Deleted directory: {}", args.path)
            } else {
                return Err(ToolExecError::Message(format!(
                    "{} is a directory. Set recursive=true to delete it.",
                    args.path
                )));
            }
        } else {
            std::fs::remove_file(&canonical)
                .map_err(|e| ToolExecError::Message(format!("cannot delete file: {e}")))?;
            format!("Deleted file: {}", args.path)
        };
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateDirectoryArgs {
    #[schemars(description = "Directory path to create")]
    pub path: String,
}

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
    type Error = ToolExecError;

    fn description(&self) -> String {
        "Create a directory and any missing parent directories (like mkdir -p)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(CreateDirectoryArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}\u{2699}  create dir {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);

        let canonical = if path.exists() {
            path.canonicalize()
                .map_err(|e| ToolExecError::Message(format!("cannot resolve path: {e}")))?
        } else {
            if let Some(parent) = path.parent() {
                if parent.exists() {
                    parent
                        .canonicalize()
                        .map_err(|e| ToolExecError::Message(format!("cannot resolve parent: {e}")))?
                        .join(path.file_name().unwrap_or_default())
                } else {
                    path.clone()
                }
            } else {
                path.clone()
            }
        };
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Write, &canonical_str) {
            return Err(ToolExecError::Message(format!(
                "write access denied for: {}",
                args.path
            )));
        }

        std::fs::create_dir_all(&canonical)
            .map_err(|e| ToolExecError::Message(format!("cannot create directory: {e}")))?;

        let result = format!("Created directory: {}", args.path);
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}
