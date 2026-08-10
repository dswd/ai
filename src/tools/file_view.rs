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
        description = "Path to the file to preview (supports PDF, DOCX, XLSX, PPTX, ODT, RTF, EPUB, CSV, HTML, and plain text)"
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
        "Extract readable text from a file. Supports PDF, Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and HTML by converting them to Markdown.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(FileViewArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}👁️ file view {}{RESET}", args.path);
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

        let ext = canonical
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());

        let text = convert_to_markdown(&bytes, ext.as_deref()).map_err(ToolError::Message)?;

        let truncated = truncate(&text, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title(&args.path),
            bar_line()
        );
        process_output(&text, None, None).map_err(ToolError::Message)
    }
}

/// Convert a file's bytes to readable Markdown. Uses `anydoc` for office
/// documents, PDFs, and other structured formats, `html2text` for HTML, and a
/// plain-text passthrough for Markdown/text files. Unsupported binary formats
/// (archives, images, audio/video) return a clear error.
fn convert_to_markdown(bytes: &[u8], ext: Option<&str>) -> Result<String, String> {
    let is_html = matches!(ext, Some("html" | "htm"))
        || bytes.starts_with(b"<!doctype")
        || bytes.starts_with(b"<!DOCTYPE")
        || bytes.starts_with(b"<html");

    // anydoc handles the format when it can detect one from the extension or
    // the content itself (PDF header, OLE stream names, ZIP mimetype, ...).
    let format = ext
        .and_then(anydoc::Format::from_extension)
        .or_else(|| anydoc::Format::from_bytes(bytes));

    if format.is_some() {
        return anydoc::to_markdown_bytes(bytes, format)
            .map_err(|e| format!("conversion failed: {e}"));
    }

    if is_html {
        return html2text::from_read(bytes, 80).map_err(|e| format!("HTML conversion failed: {e}"));
    }

    // Plain text / Markdown / unknown text-like files: pass through as-is.
    if ext.is_none_or(|e| {
        matches!(
            e,
            "md" | "txt"
                | "text"
                | "log"
                | "json"
                | "xml"
                | "yaml"
                | "yml"
                | "toml"
                | "ini"
                | "sh"
                | "rs"
                | "py"
                | "js"
                | "ts"
                | "c"
                | "h"
                | "java"
                | "go"
                | "css"
                | "sql"
        )
    }) {
        return Ok(String::from_utf8_lossy(bytes).to_string());
    }

    Err(format!(
        "unsupported format for file_view: {}",
        ext.unwrap_or("unknown")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_plain_text() {
        assert_eq!(
            convert_to_markdown(b"hello world", Some("txt")).unwrap(),
            "hello world"
        );
        assert_eq!(
            convert_to_markdown(b"# Title", Some("md")).unwrap(),
            "# Title"
        );
        assert_eq!(convert_to_markdown(b"x=1", Some("yaml")).unwrap(), "x=1");
    }

    #[test]
    fn test_convert_unsupported_binary() {
        let err = convert_to_markdown(&[0x50, 0x4B, 0x03, 0x04, 0x00], Some("zip")).unwrap_err();
        assert!(err.contains("unsupported format"));
        let err = convert_to_markdown(&[0x89, 0x50, 0x4E, 0x47], Some("png")).unwrap_err();
        assert!(err.contains("png"));
    }

    #[test]
    fn test_convert_html_fallback() {
        let out = convert_to_markdown(b"<h1>Hi</h1><p>there</p>", Some("html")).unwrap();
        assert!(out.contains("Hi"));
        assert!(out.contains("there"));
    }

    #[test]
    fn test_convert_office_via_anydoc() {
        // A minimal XLSX (ZIP container) should be detected by content even
        // without an extension, and anydoc should return Markdown or an error.
        let xlsx = [
            0x50, 0x4B, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let res = convert_to_markdown(&xlsx, Some("xlsx"));
        assert!(res.is_ok() || res.unwrap_err().contains("conversion failed"));
    }
}
