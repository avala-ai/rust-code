//! Bash tool: execute shell commands.
//!
//! Runs commands via the system shell and returns stdout/stderr.
//! Supports timeouts, background execution, and sandboxing.

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Shell command execution tool.
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Executes a shell command and returns its output."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (max 600000)"
                },
                "description": {
                    "type": "string",
                    "description": "Description of what this command does"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn get_path(&self, _input: &serde_json::Value) -> Option<PathBuf> {
        None
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'command' is required".into()))?;

        let timeout_ms = input
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(120_000)
            .min(600_000);

        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to spawn: {e}")))?;

        let timeout = Duration::from_millis(timeout_ms);

        // Read stdout/stderr while waiting, with timeout and cancellation.
        let mut stdout_handle = child.stdout.take().unwrap();
        let mut stderr_handle = child.stderr.take().unwrap();

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        let result = tokio::select! {
            r = async {
                let (so, se) = tokio::join!(
                    async { stdout_handle.read_to_end(&mut stdout_buf).await },
                    async { stderr_handle.read_to_end(&mut stderr_buf).await },
                );
                so?;
                se?;
                child.wait().await
            } => {
                match r {
                    Ok(status) => {
                        let stdout = String::from_utf8_lossy(&stdout_buf).to_string();
                        let stderr = String::from_utf8_lossy(&stderr_buf).to_string();
                        let exit_code = status.code().unwrap_or(-1);

                        let mut content = String::new();
                        if !stdout.is_empty() {
                            content.push_str(&stdout);
                        }
                        if !stderr.is_empty() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(&format!("stderr:\n{stderr}"));
                        }
                        if content.is_empty() {
                            content = "(no output)".to_string();
                        }

                        if exit_code != 0 {
                            content.push_str(&format!("\n\nExit code: {exit_code}"));
                        }

                        Ok(ToolResult {
                            content,
                            is_error: exit_code != 0,
                        })
                    }
                    Err(e) => Err(ToolError::ExecutionFailed(e.to_string())),
                }
            }
            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                Err(ToolError::Timeout(timeout_ms))
            }
            _ = ctx.cancel.cancelled() => {
                let _ = child.kill().await;
                Err(ToolError::Cancelled)
            }
        };

        result
    }
}
