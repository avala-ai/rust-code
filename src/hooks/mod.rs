//! Hook system.
//!
//! Hooks allow user-defined actions to run at specific points in the
//! agent lifecycle:
//!
//! - `PreToolUse` — before a tool executes (can block/modify)
//! - `PostToolUse` — after a tool completes
//! - `SessionStart` — when a session begins
//! - `SessionStop` — when a session ends
//! - `UserPromptSubmit` — when the user submits input
//!
//! Hooks can be shell commands, HTTP endpoints, or prompt templates,
//! configured in the settings file.

use serde::{Deserialize, Serialize};

/// Hook event types that can trigger user-defined actions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    SessionStart,
    SessionStop,
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
}

/// A configured hook action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HookAction {
    /// Run a shell command.
    #[serde(rename = "shell")]
    Shell { command: String },
    /// Make an HTTP request.
    #[serde(rename = "http")]
    Http { url: String, method: Option<String> },
}

/// A hook definition binding an event to an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    pub event: HookEvent,
    pub action: HookAction,
    /// Optional tool name filter (for PreToolUse/PostToolUse).
    pub tool_name: Option<String>,
}

/// Hook registry that stores and dispatches hooks.
pub struct HookRegistry {
    hooks: Vec<HookDefinition>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, hook: HookDefinition) {
        self.hooks.push(hook);
    }

    /// Get all hooks for a given event, optionally filtered by tool name.
    pub fn get_hooks(&self, event: &HookEvent, tool_name: Option<&str>) -> Vec<&HookDefinition> {
        self.hooks
            .iter()
            .filter(|h| {
                h.event == *event
                    && (h.tool_name.is_none()
                        || tool_name.is_none()
                        || h.tool_name.as_deref() == tool_name)
            })
            .collect()
    }

    /// Execute all hooks for a given event. Shell hooks run as subprocesses.
    pub async fn run_hooks(
        &self,
        event: &HookEvent,
        tool_name: Option<&str>,
        _context: &serde_json::Value,
    ) -> Vec<HookResult> {
        let hooks = self.get_hooks(event, tool_name);
        let mut results = Vec::new();

        for hook in hooks {
            let result = match &hook.action {
                HookAction::Shell { command } => {
                    match tokio::process::Command::new("bash")
                        .arg("-c")
                        .arg(command)
                        .output()
                        .await
                    {
                        Ok(output) => HookResult {
                            success: output.status.success(),
                            output: String::from_utf8_lossy(&output.stdout).to_string(),
                        },
                        Err(e) => HookResult {
                            success: false,
                            output: e.to_string(),
                        },
                    }
                }
                HookAction::Http { url, method } => {
                    let client = reqwest::Client::new();
                    let method = method.as_deref().unwrap_or("POST");
                    let req = match method {
                        "GET" => client.get(url),
                        _ => client.post(url),
                    };
                    match req.send().await {
                        Ok(resp) => HookResult {
                            success: resp.status().is_success(),
                            output: resp.text().await.unwrap_or_default(),
                        },
                        Err(e) => HookResult {
                            success: false,
                            output: e.to_string(),
                        },
                    }
                }
            };
            results.push(result);
        }

        results
    }
}

/// Result of executing a hook.
#[derive(Debug)]
pub struct HookResult {
    pub success: bool,
    pub output: String,
}
