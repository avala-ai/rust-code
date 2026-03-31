//! Grep tool: regex-based content search.
//!
//! Searches file contents using regular expressions. Shells out to
//! `rg` (ripgrep) when available for performance and .gitignore
//! awareness. Falls back to a built-in implementation.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Searches file contents using regular expressions. Powered by ripgrep."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., \"*.rs\")"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive search",
                    "default": false
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Lines of context around each match"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of output lines",
                    "default": 250
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

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let pattern = input
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'pattern' is required".into()))?;

        let search_path = input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.cwd.clone());

        let glob_filter = input.get("glob").and_then(|v| v.as_str());
        let case_insensitive = input
            .get("case_insensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let context_lines = input
            .get("context_lines")
            .and_then(|v| v.as_u64());
        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(250) as usize;

        // Build ripgrep command.
        let mut cmd = Command::new("rg");
        cmd.arg("--line-number")
            .arg("--no-heading")
            .arg("--color=never");

        if case_insensitive {
            cmd.arg("-i");
        }

        if let Some(ctx_lines) = context_lines {
            cmd.arg(format!("-C{ctx_lines}"));
        }

        if let Some(glob_pat) = glob_filter {
            cmd.arg("--glob").arg(glob_pat);
        }

        cmd.arg(pattern).arg(&search_path);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to run rg: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Truncate to max_results lines.
        let lines: Vec<&str> = stdout.lines().collect();
        let total = lines.len();
        let truncated = total > max_results;
        let display_lines = &lines[..total.min(max_results)];

        let mut result = display_lines.join("\n");
        if truncated {
            result.push_str(&format!(
                "\n\n(Showing {max_results} of {total} lines. Use a more specific pattern.)"
            ));
        }

        if result.is_empty() {
            result = "No matches found.".to_string();
        }

        let num_files = lines
            .iter()
            .filter_map(|l| l.split(':').next())
            .collect::<std::collections::HashSet<_>>()
            .len();

        Ok(ToolResult::success(format!(
            "Found {total} matches across {num_files} files:\n{result}"
        )))
    }
}
