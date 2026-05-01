//! `Dream` executor — idle-time background work.
//!
//! Stub: the idle-time scheduler that picks up `Dream` tasks lands
//! later. The executor exists so the kind is reachable through the
//! registry and round-trips through serde.

use async_trait::async_trait;

use crate::services::background::{TaskKind, TaskPayload};
use crate::tools::tasks::executor::{TaskContext, TaskError, TaskExecutor, TaskResult};

pub struct DreamExecutor;

#[async_trait]
impl TaskExecutor for DreamExecutor {
    fn kind(&self) -> TaskKind {
        TaskKind::Dream
    }

    async fn execute(
        &self,
        payload: &TaskPayload,
        _ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        match payload {
            TaskPayload::Dream { .. } => Err(TaskError::NotImplemented("Dream")),
            other => Err(TaskError::PayloadMismatch {
                expected: TaskKind::Dream,
                actual: other.kind(),
            }),
        }
    }
}
