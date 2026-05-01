//! Background task execution.
//!
//! Manages tasks that run asynchronously while the user continues
//! interacting with the agent. Tasks output to files and notify
//! the user when complete.
//!
//! # Task model
//!
//! Tasks are kind-tagged (see [`TaskKind`]) so the same queue can
//! carry shell commands, subagent runs, MCP monitors, and idle-time
//! "dream" jobs. Each kind carries kind-specific data in
//! [`TaskPayload`]; the [`crate::tools::tasks::executor`] module
//! defines the per-kind executor trait and registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, info};

/// Unique task identifier.
pub type TaskId = String;

/// Status of a background task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed(String),
    Killed,
}

/// What kind of work a task represents.
///
/// All kinds share the same queue, persistence layout, and lifecycle —
/// the kind determines which executor runs the work and how the
/// kind-specific [`TaskPayload`] is interpreted.
///
/// `LocalShell` is the legacy default: a record without a `kind`
/// field on disk deserializes as `LocalShell` so older state files
/// keep working. See [`TaskKind::default`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// User-issued shell command run via the Bash tool.
    ///
    /// Marked `#[default]` so legacy task records (pre-8.13) without
    /// a `kind` field round-trip cleanly through serde.
    #[default]
    LocalShell,
    /// An Agent-tool subagent run.
    LocalAgent,
    /// A multi-step skill / workflow run.
    LocalWorkflow,
    /// A "watch an MCP server" task.
    MonitorMcp,
    /// A cloud-runtime / RemoteTrigger run. Stub for 8.14.
    RemoteAgent,
    /// Idle-time background work.
    Dream,
}

impl TaskKind {
    /// Stable, human-friendly label used in `/tasks` output and
    /// surfaced through the `TaskList` / `TaskGet` tool results.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::LocalShell => "LocalShell",
            Self::LocalAgent => "LocalAgent",
            Self::LocalWorkflow => "LocalWorkflow",
            Self::MonitorMcp => "MonitorMcp",
            Self::RemoteAgent => "RemoteAgent",
            Self::Dream => "Dream",
        }
    }

    /// Parse a kind from its `as_str` form (case-insensitive). Used by
    /// `TaskCreate` so the model can pass `"local_agent"` / `"LocalAgent"`
    /// interchangeably.
    pub fn parse(s: &str) -> Option<Self> {
        let normalized = s.replace('_', "").to_ascii_lowercase();
        match normalized.as_str() {
            "localshell" => Some(Self::LocalShell),
            "localagent" => Some(Self::LocalAgent),
            "localworkflow" => Some(Self::LocalWorkflow),
            "monitormcp" => Some(Self::MonitorMcp),
            "remoteagent" => Some(Self::RemoteAgent),
            "dream" => Some(Self::Dream),
            _ => None,
        }
    }
}

/// Kind-specific data carried alongside a task record.
///
/// Serialized with the standard tagged-enum form so the persisted
/// shape is `{ "kind": "local_agent", "payload": { ... } }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum TaskPayload {
    /// A shell command launched via the Bash tool.
    LocalShell {
        /// Command to run.
        command: String,
        /// Working directory the command was launched from.
        cwd: PathBuf,
    },
    /// A subagent run dispatched through the Agent tool.
    LocalAgent {
        /// Optional subagent kind (e.g. a named agent profile).
        subagent_kind: Option<String>,
        /// Prompt the subagent should execute.
        prompt: String,
        /// Parent session id, when one is available.
        parent_session: Option<String>,
    },
    /// A multi-step skill / workflow execution.
    LocalWorkflow {
        /// Slug of the skill / workflow to run.
        workflow: String,
        /// Free-form arguments forwarded to the workflow.
        args: serde_json::Value,
    },
    /// Watch an MCP server for events.
    MonitorMcp {
        /// Configured MCP server name.
        server_name: String,
        /// Optional tool the watcher expects to fire.
        expected_tool: Option<String>,
        /// How long to keep watching before giving up.
        timeout: Duration,
    },
    /// A cloud-runtime / RemoteTrigger run. Stub for 8.14.
    RemoteAgent {
        /// Stored routine id to trigger.
        routine_id: String,
        /// Optional wall-clock cap on the run.
        timeout: Option<Duration>,
    },
    /// Idle-time background work. Free-form payload so the dream
    /// executor can stash whatever signal it needs to resume.
    Dream { note: Option<String> },
}

impl TaskPayload {
    /// Map a payload back to its [`TaskKind`].
    pub fn kind(&self) -> TaskKind {
        match self {
            Self::LocalShell { .. } => TaskKind::LocalShell,
            Self::LocalAgent { .. } => TaskKind::LocalAgent,
            Self::LocalWorkflow { .. } => TaskKind::LocalWorkflow,
            Self::MonitorMcp { .. } => TaskKind::MonitorMcp,
            Self::RemoteAgent { .. } => TaskKind::RemoteAgent,
            Self::Dream { .. } => TaskKind::Dream,
        }
    }
}

/// Metadata for a running or completed background task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: TaskId,
    pub description: String,
    pub status: TaskStatus,
    pub output_file: PathBuf,
    /// What kind of work this task represents. Defaults to
    /// `LocalShell` so records persisted before the kind field existed
    /// continue to round-trip through serde.
    #[serde(default)]
    pub kind: TaskKind,
    /// Kind-specific payload. `None` is the documented default for
    /// legacy records — the task simply has no extra data attached.
    #[serde(default)]
    pub payload: Option<TaskPayload>,
    /// Wall-clock instants are not portable across processes; skip
    /// them in the persisted form.
    #[serde(skip, default = "std::time::Instant::now")]
    pub started_at: std::time::Instant,
    #[serde(skip, default)]
    pub finished_at: Option<std::time::Instant>,
}

/// Manages background task lifecycle.
pub struct TaskManager {
    tasks: Arc<Mutex<HashMap<TaskId, TaskInfo>>>,
    next_id: Arc<Mutex<u64>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    /// Spawn a background shell command.
    pub async fn spawn_shell(
        &self,
        command: &str,
        description: &str,
        cwd: &Path,
    ) -> Result<TaskId, String> {
        let id = self.allocate_id("b").await;
        let output_file = task_output_path(&id);

        // Ensure output directory exists.
        if let Some(parent) = output_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let info = TaskInfo {
            id: id.clone(),
            description: description.to_string(),
            status: TaskStatus::Running,
            output_file: output_file.clone(),
            kind: TaskKind::LocalShell,
            payload: Some(TaskPayload::LocalShell {
                command: command.to_string(),
                cwd: cwd.to_path_buf(),
            }),
            started_at: std::time::Instant::now(),
            finished_at: None,
        };

        self.tasks.lock().await.insert(id.clone(), info);

        // Spawn the process.
        let task_id = id.clone();
        let tasks = self.tasks.clone();
        let command = command.to_string();
        let cwd = cwd.to_path_buf();

        tokio::spawn(async move {
            let result = tokio::process::Command::new("bash")
                .arg("-c")
                .arg(&command)
                .current_dir(&cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await;

            let mut tasks = tasks.lock().await;
            if let Some(info) = tasks.get_mut(&task_id) {
                info.finished_at = Some(std::time::Instant::now());

                match result {
                    Ok(output) => {
                        let mut content = String::new();
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if !stdout.is_empty() {
                            content.push_str(&stdout);
                        }
                        if !stderr.is_empty() {
                            content.push_str("\nstderr:\n");
                            content.push_str(&stderr);
                        }
                        let _ = std::fs::write(&info.output_file, &content);

                        if output.status.success() {
                            info.status = TaskStatus::Completed;
                        } else {
                            info.status = TaskStatus::Failed(format!(
                                "Exit code: {}",
                                output.status.code().unwrap_or(-1)
                            ));
                        }
                    }
                    Err(e) => {
                        info.status = TaskStatus::Failed(e.to_string());
                        let _ = std::fs::write(&info.output_file, e.to_string());
                    }
                }

                info!("Background task {} finished: {:?}", task_id, info.status);
            }
        });

        debug!("Background task {id} started: {description}");
        Ok(id)
    }

    /// Register a non-shell task in the queue.
    ///
    /// Used by the kind-specific executors (`LocalAgent`, `MonitorMcp`,
    /// etc.) — they handle their own runtime, but still want a queue
    /// entry so the task is visible to `/tasks`, `TaskList`, and
    /// `TaskGet`. The caller is responsible for transitioning the
    /// status when the work finishes; see [`Self::set_status`].
    pub async fn register(
        &self,
        description: &str,
        kind: TaskKind,
        payload: TaskPayload,
    ) -> TaskId {
        let id = self.allocate_id(id_prefix_for(kind)).await;
        let output_file = task_output_path(&id);
        if let Some(parent) = output_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let info = TaskInfo {
            id: id.clone(),
            description: description.to_string(),
            status: TaskStatus::Running,
            output_file,
            kind,
            payload: Some(payload),
            started_at: std::time::Instant::now(),
            finished_at: None,
        };
        self.tasks.lock().await.insert(id.clone(), info);
        debug!("Registered {kind:?} task {id}: {description}");
        id
    }

    /// Update the status of an existing task. Used by the kind-specific
    /// executors when their externally-driven work completes.
    pub async fn set_status(&self, id: &str, status: TaskStatus) -> Result<(), String> {
        let mut tasks = self.tasks.lock().await;
        let info = tasks
            .get_mut(id)
            .ok_or_else(|| format!("Task '{id}' not found"))?;
        let now_finished = matches!(
            status,
            TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Killed,
        );
        info.status = status;
        if now_finished && info.finished_at.is_none() {
            info.finished_at = Some(std::time::Instant::now());
        }
        Ok(())
    }

    /// Get the status of a task.
    pub async fn get_status(&self, id: &str) -> Option<TaskInfo> {
        self.tasks.lock().await.get(id).cloned()
    }

    /// Read the output of a completed task.
    pub async fn read_output(&self, id: &str) -> Result<String, String> {
        let tasks = self.tasks.lock().await;
        let info = tasks
            .get(id)
            .ok_or_else(|| format!("Task '{id}' not found"))?;
        std::fs::read_to_string(&info.output_file)
            .map_err(|e| format!("Failed to read output: {e}"))
    }

    /// List all tasks.
    pub async fn list(&self) -> Vec<TaskInfo> {
        self.tasks.lock().await.values().cloned().collect()
    }

    /// Kill a running task (best-effort).
    pub async fn kill(&self, id: &str) -> Result<(), String> {
        let mut tasks = self.tasks.lock().await;
        let info = tasks
            .get_mut(id)
            .ok_or_else(|| format!("Task '{id}' not found"))?;
        if info.status == TaskStatus::Running {
            info.status = TaskStatus::Killed;
            info.finished_at = Some(std::time::Instant::now());
        }
        Ok(())
    }

    /// Collect notifications for newly completed tasks.
    pub async fn drain_completions(&self) -> Vec<TaskInfo> {
        let tasks = self.tasks.lock().await;
        tasks
            .values()
            .filter(|t| matches!(t.status, TaskStatus::Completed | TaskStatus::Failed(_)))
            .cloned()
            .collect()
    }

    async fn allocate_id(&self, prefix: &str) -> TaskId {
        let mut next = self.next_id.lock().await;
        let id = format!("{prefix}{next}");
        *next += 1;
        id
    }
}

/// Two-letter id prefix per kind so the `/tasks` table tells them
/// apart at a glance.
const fn id_prefix_for(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::LocalShell => "b",
        TaskKind::LocalAgent => "a",
        TaskKind::LocalWorkflow => "w",
        TaskKind::MonitorMcp => "m",
        TaskKind::RemoteAgent => "r",
        TaskKind::Dream => "d",
    }
}

/// Path where task output is stored.
fn task_output_path(id: &TaskId) -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-code")
        .join("tasks");
    dir.join(format!("{id}.out"))
}
