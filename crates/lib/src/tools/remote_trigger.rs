//! RemoteTrigger tool: fire a one-off run of a stored routine.
//!
//! The schedule executor needs an LLM provider and full config to drive
//! a turn, neither of which the per-tool `ToolContext` carries. We mirror
//! the [`crate::tools::agent::AgentTool`] pattern and spawn the host
//! `agent schedule run <id>` subprocess so the routine runs through the
//! same code path as `agent schedule run`. The tool waits for the
//! subprocess to finish (subject to an optional timeout) and returns its
//! captured output, keeping the call request/response in spirit.

use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;
use crate::permissions::{PermissionChecker, PermissionDecision};

use super::cron_support::open_store;

/// Default wall-clock cap for a remote-triggered run. Keeps the tool
/// call from hanging indefinitely if the routine wedges. Callers can
/// raise (or lower) this via `timeout_seconds`.
const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Hard ceiling — even when callers ask for longer, we cap here so a
/// runaway routine can't hold the tool open forever.
const MAX_TIMEOUT_SECS: u64 = 3600;

pub struct RemoteTriggerTool;

#[async_trait]
impl Tool for RemoteTriggerTool {
    fn name(&self) -> &'static str {
        "RemoteTrigger"
    }

    fn description(&self) -> &'static str {
        "Fire a one-off run of a stored cron routine and return its output. \
         Blocks until the routine finishes or the timeout elapses."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Routine id to trigger (as returned by CronCreate or CronList)."
                },
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 3600,
                    "description": "Optional wall-clock timeout for the run. Defaults to 600 seconds, capped at 3600."
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_destructive(&self) -> bool {
        // Triggering a run consumes API budget and may mutate the
        // working directory, so gate it on the standard permission
        // checker rather than auto-allowing.
        true
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        checker: &PermissionChecker,
    ) -> PermissionDecision {
        checker.check(self.name(), input)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'id' is required".into()))?;

        // Verify the routine exists before forking — gives the model a
        // crisp error rather than a subprocess failure code.
        let store = open_store()
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to open schedule store: {e}")))?;
        if store.load(id).is_err() {
            return Err(ToolError::InvalidInput(format!(
                "No routine with id '{id}' exists. Use CronList to see available routines."
            )));
        }

        let timeout_secs = input
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        // Spawn `agent schedule run <id>` to delegate to the existing
        // executor. The subprocess inherits provider env vars; the
        // routine record itself supplies cwd, model, and prompt.
        let agent_binary = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "agent".to_string());

        let mut cmd = tokio::process::Command::new(&agent_binary);
        cmd.arg("schedule")
            .arg("run")
            .arg(id)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Forward common provider env vars so the subprocess can
        // authenticate without re-reading config.
        for var in &[
            "AGENT_CODE_API_KEY",
            "AGENT_CODE_API_BASE_URL",
            "AGENT_CODE_MODEL",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "XAI_API_KEY",
            "GOOGLE_API_KEY",
            "DEEPSEEK_API_KEY",
            "GROQ_API_KEY",
            "MISTRAL_API_KEY",
            "TOGETHER_API_KEY",
            crate::tools::cron_support::SCHEDULES_DIR_ENV,
        ] {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let result = tokio::select! {
            r = cmd.output() => r,
            _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                return Err(ToolError::Timeout(timeout_secs * 1_000));
            }
            _ = ctx.cancel.cancelled() => {
                return Err(ToolError::Cancelled);
            }
        };

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let success = output.status.success();

                let mut content = format!(
                    "Routine '{id}' triggered (exit={}).\n",
                    output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                );
                if !stdout.is_empty() {
                    content.push_str("\n--- stdout ---\n");
                    content.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    content.push_str("\n--- stderr ---\n");
                    content.push_str(&stderr);
                }

                Ok(ToolResult {
                    content,
                    is_error: !success,
                })
            }
            Err(e) => Err(ToolError::ExecutionFailed(format!(
                "Failed to spawn '{}' schedule run {id}: {e}",
                agent_binary
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::cron_support::{test_ctx, with_test_store};

    #[tokio::test]
    async fn trigger_rejects_unknown_routine() {
        let _guard = with_test_store();
        let err = RemoteTriggerTool
            .call(json!({"id": "missing"}), &test_ctx())
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[tokio::test]
    async fn trigger_requires_id() {
        let _guard = with_test_store();
        let err = RemoteTriggerTool
            .call(json!({}), &test_ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
