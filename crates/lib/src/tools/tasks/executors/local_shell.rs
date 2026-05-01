//! `LocalShell` executor — runs a user-issued shell command.
//!
//! This wraps the same path that the Bash tool's background mode uses:
//! [`TaskManager::spawn_shell`](crate::services::background::TaskManager::spawn_shell).
//! The executor itself does not duplicate the spawn logic — it only
//! validates the payload and threads it through.

use async_trait::async_trait;

use crate::services::background::{TaskKind, TaskManager, TaskPayload};
use crate::tools::tasks::executor::{TaskContext, TaskError, TaskExecutor, TaskResult};

pub struct LocalShellExecutor;

#[async_trait]
impl TaskExecutor for LocalShellExecutor {
    fn kind(&self) -> TaskKind {
        TaskKind::LocalShell
    }

    async fn execute(
        &self,
        payload: &TaskPayload,
        ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        let (command, cwd) = match payload {
            TaskPayload::LocalShell { command, cwd } => (command.as_str(), cwd.clone()),
            other => {
                return Err(TaskError::PayloadMismatch {
                    expected: TaskKind::LocalShell,
                    actual: other.kind(),
                });
            }
        };

        // Use the caller-provided manager so the spawned record shares
        // the same queue the rest of the agent sees. Fall back to a
        // throwaway manager only when no shared one was wired in
        // (mainly for unit tests).
        let owned_mgr = TaskManager::new();
        let mgr = ctx.task_manager.as_deref().unwrap_or(&owned_mgr);

        let id = mgr
            .spawn_shell(command, command, &cwd)
            .await
            .map_err(TaskError::ExecutionFailed)?;

        Ok(TaskResult::success(format!(
            "Shell task {id} spawned: {command}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn rejects_wrong_payload_kind() {
        let exec = LocalShellExecutor;
        let ctx = TaskContext::new(PathBuf::from("/tmp"));
        let payload = TaskPayload::Dream { note: None };
        let err = exec.execute(&payload, &ctx).await.unwrap_err();
        match err {
            TaskError::PayloadMismatch { expected, actual } => {
                assert_eq!(expected, TaskKind::LocalShell);
                assert_eq!(actual, TaskKind::Dream);
            }
            other => panic!("unexpected err: {other:?}"),
        }
    }
}
