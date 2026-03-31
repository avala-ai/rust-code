//! FileRead tool: read file contents with optional line ranges.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &'static str {
        "FileRead"
    }

    fn description(&self) -> &'static str {
        "Reads a file from the filesystem. Returns contents with line numbers."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    fn get_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'file_path' is required".into()))?;

        let offset = input
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;

        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(2000) as usize;

        let content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read {file_path}: {e}")))?;

        // Apply line range and add line numbers (1-indexed, matching `cat -n`).
        let lines: Vec<&str> = content.lines().collect();
        let start = (offset.saturating_sub(1)).min(lines.len());
        let end = (start + limit).min(lines.len());

        let mut output = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_num = start + i + 1;
            output.push_str(&format!("{line_num}\t{line}\n"));
        }

        if output.is_empty() {
            output = "(empty file)".to_string();
        }

        Ok(ToolResult::success(output))
    }
}
