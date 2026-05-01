//! `LocalWorkflow` executor — runs a multi-step skill / workflow.
//!
//! Stub: the workflow runtime is part of the larger skills/epic
//! workstream. The executor still validates the payload so callers
//! get a clear error rather than the registry's generic "no executor"
//! message when they wire one up before that work lands.

use async_trait::async_trait;

use crate::services::background::{TaskKind, TaskPayload};
use crate::tools::tasks::executor::{TaskContext, TaskError, TaskExecutor, TaskResult};

pub struct LocalWorkflowExecutor;

#[async_trait]
impl TaskExecutor for LocalWorkflowExecutor {
    fn kind(&self) -> TaskKind {
        TaskKind::LocalWorkflow
    }

    async fn execute(
        &self,
        payload: &TaskPayload,
        _ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        // TODO(8.x): hook into the skills runtime once multi-step
        // skill execution is unified with the task queue.
        match payload {
            TaskPayload::LocalWorkflow { workflow, .. } => {
                if workflow.trim().is_empty() {
                    return Err(TaskError::InvalidPayload(
                        "LocalWorkflow payload requires a workflow slug".into(),
                    ));
                }
                Err(TaskError::NotImplemented("LocalWorkflow"))
            }
            other => Err(TaskError::PayloadMismatch {
                expected: TaskKind::LocalWorkflow,
                actual: other.kind(),
            }),
        }
    }
}
