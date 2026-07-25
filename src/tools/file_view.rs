use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, process_output, truncate};
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileViewArgs {
    #[schemars(
        description = "Path to the file to preview (supports PDF, DOCX, XLSX, PPTX, HTML, CSV, XML, images, ZIP)"
    )]
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct FileViewTool {
    policy: Policy,
}

impl FileViewTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for FileViewTool {
    const NAME: &'static str = "file_view";

    type Args = FileViewArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Extract readable text from a file. Supports PDF, Word, Excel, PowerPoint, HTML, CSV, XML, images, and ZIP archives by converting them to Markdown.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(FileViewArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = PathBuf::from(&args.path);
        let canonical = path
            .canonicalize()
            .map_err(|e| ToolError::Message(format!("cannot resolve path: {e}")))?;
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Read, &canonical_str) {
            return Err(ToolError::Message(format!(
                "read access denied for: {canonical_str}"
            )));
        }

        let bytes = std::fs::read(&canonical)
            .map_err(|e| ToolError::Message(format!("cannot read file: {e}")))?;

        let md = markitdown::MarkItDown::new();
        let result = md
            .convert_bytes(
                &bytes,
                Some(markitdown::model::ConversionOptions {
                    file_extension: canonical
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| format!(".{e}")),
                    url: None,
                    llm_client: None,
                    llm_model: None,
                }),
            )
            .map_err(|e| ToolError::Message(format!("conversion failed: {e}")))?;

        match result {
            Some(r) => {
                info!("{DIM}👁️ file_view {}{RESET}", args.path);
                let truncated = truncate(&r.text_content, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
                debug!(
                    "{DIM} {} \n{truncated}\n {} {RESET}",
                    bar_title(&args.path),
                    bar_line()
                );
                process_output(&r.text_content, None, None).map_err(ToolError::Message)
            }
            None => Err(ToolError::Message(
                "unsupported file type — cannot extract text".to_string(),
            )),
        }
    }
}
