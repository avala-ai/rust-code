//! `LocalAgent` executor — runs a subagent through the existing
//! [`AgentTool`](crate::tools::agent::AgentTool).
//!
//! We deliberately do not reimplement subagent spawning here. The
//! executor builds the JSON input the `Agent` tool already accepts
//! and forwards the call. That keeps worktree isolation, environment
//! plumbing, and the timeout semantics in one place.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use crate::permissions::PermissionChecker;
use crate::services::background::{TaskKind, TaskPayload};
use crate::tools::agent::AgentTool;
use crate::tools::tasks::executor::{TaskContext, TaskError, TaskExecutor, TaskResult};
use crate::tools::{Tool, ToolContext};

pub struct LocalAgentExecutor;

#[async_trait]
impl TaskExecutor for LocalAgentExecutor {
    fn kind(&self) -> TaskKind {
        TaskKind::LocalAgent
    }

    async fn execute(
        &self,
        payload: &TaskPayload,
        ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        let (subagent_kind, prompt) = match payload {
            TaskPayload::LocalAgent {
                subagent_kind,
                prompt,
                ..
            } => (subagent_kind.clone(), prompt.clone()),
            other => {
                return Err(TaskError::PayloadMismatch {
                    expected: TaskKind::LocalAgent,
                    actual: other.kind(),
                });
            }
        };

        if prompt.trim().is_empty() {
            return Err(TaskError::InvalidPayload(
                "LocalAgent payload requires a non-empty prompt".into(),
            ));
        }

        // Map our payload onto the input shape the Agent tool already
        // understands. Reuse, don't reimplement.
        let description = subagent_kind.unwrap_or_else(|| "subagent".to_string());
        let input = json!({
            "description": description,
            "prompt": prompt,
        });

        // Build a minimal tool context. The Agent tool spawns a child
        // `agent` subprocess so it does not reach into permissions or
        // the file cache for itself — defaults are fine.
        let tool_ctx = ToolContext {
            cwd: ctx.cwd.clone(),
            cancel: ctx.cancel.clone(),
            permission_checker: Arc::new(PermissionChecker::allow_all()),
            verbose: false,
            plan_mode: false,
            file_cache: None,
            denial_tracker: None,
            task_manager: ctx.task_manager.clone(),
            session_allows: None,
            permission_prompter: None,
            sandbox: None,
            active_disk_output_style: None,
        };

        match AgentTool.call(input, &tool_ctx).await {
            Ok(result) => Ok(TaskResult {
                output: result.content,
                is_error: result.is_error,
            }),
            Err(crate::error::ToolError::Cancelled) => Err(TaskError::Cancelled),
            Err(e) => Err(TaskError::ExecutionFailed(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn rejects_empty_prompt() {
        let exec = LocalAgentExecutor;
        let ctx = TaskContext::new(PathBuf::from("/tmp"));
        let payload = TaskPayload::LocalAgent {
            subagent_kind: None,
            prompt: "   ".into(),
            parent_session: None,
        };
        let err = exec.execute(&payload, &ctx).await.unwrap_err();
        assert!(matches!(err, TaskError::InvalidPayload(_)));
    }

    #[tokio::test]
    async fn rejects_wrong_payload_kind() {
        let exec = LocalAgentExecutor;
        let ctx = TaskContext::new(PathBuf::from("/tmp"));
        let payload = TaskPayload::Dream { note: None };
        let err = exec.execute(&payload, &ctx).await.unwrap_err();
        assert!(matches!(err, TaskError::PayloadMismatch { .. }));
    }
}
