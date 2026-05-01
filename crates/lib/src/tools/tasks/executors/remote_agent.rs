//! `RemoteAgent` executor — placeholder for the 8.14 cloud-runtime path.
//!
//! Today the closest analogue is the local
//! [`RemoteTrigger`](crate::tools::remote_trigger::RemoteTriggerTool)
//! tool, which fires a stored cron routine on this machine. The full
//! cloud-runtime executor lands in 8.14 — until then we surface
//! `NotImplemented` so downstream callers know not to lean on this
//! kind yet, but keep the type plumbing in place.

use async_trait::async_trait;

use crate::services::background::{TaskKind, TaskPayload};
use crate::tools::tasks::executor::{TaskContext, TaskError, TaskExecutor, TaskResult};

pub struct RemoteAgentExecutor;

#[async_trait]
impl TaskExecutor for RemoteAgentExecutor {
    fn kind(&self) -> TaskKind {
        TaskKind::RemoteAgent
    }

    async fn execute(
        &self,
        payload: &TaskPayload,
        _ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        // TODO(8.14): once the cloud-runtime wire protocol lands,
        // dispatch through it instead of returning NotImplemented.
        // For local-only setups, callers should use the
        // `RemoteTrigger` tool directly.
        match payload {
            TaskPayload::RemoteAgent { routine_id, .. } => {
                if routine_id.trim().is_empty() {
                    return Err(TaskError::InvalidPayload(
                        "RemoteAgent payload requires a routine_id".into(),
                    ));
                }
                Err(TaskError::NotImplemented("RemoteAgent"))
            }
            other => Err(TaskError::PayloadMismatch {
                expected: TaskKind::RemoteAgent,
                actual: other.kind(),
            }),
        }
    }
}
