//! `MonitorMcp` executor — watch an MCP server for events.
//!
//! Stub: the watcher loop and event-bridge plumbing land in a later
//! phase. The executor returns `NotImplemented` so callers can wire
//! up the type plumbing today and slot the runtime in once it lands.

use async_trait::async_trait;

use crate::services::background::{TaskKind, TaskPayload};
use crate::tools::tasks::executor::{TaskContext, TaskError, TaskExecutor, TaskResult};

pub struct MonitorMcpExecutor;

#[async_trait]
impl TaskExecutor for MonitorMcpExecutor {
    fn kind(&self) -> TaskKind {
        TaskKind::MonitorMcp
    }

    async fn execute(
        &self,
        payload: &TaskPayload,
        _ctx: &TaskContext,
    ) -> Result<TaskResult, TaskError> {
        // TODO(8.x): drive a long-running watcher against
        // `services::mcp` and surface tool-call events back through
        // the task output stream.
        match payload {
            TaskPayload::MonitorMcp { server_name, .. } => {
                if server_name.trim().is_empty() {
                    return Err(TaskError::InvalidPayload(
                        "MonitorMcp payload requires a server_name".into(),
                    ));
                }
                Err(TaskError::NotImplemented("MonitorMcp"))
            }
            other => Err(TaskError::PayloadMismatch {
                expected: TaskKind::MonitorMcp,
                actual: other.kind(),
            }),
        }
    }
}
