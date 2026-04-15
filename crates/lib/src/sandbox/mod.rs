//! Process-level sandboxing for subprocess-spawning tools.
//!
//! Sandboxing wraps [`tokio::process::Command`] with an OS-level isolation
//! mechanism before the child is spawned. The current permission system is
//! policy enforced *inside* the agent process — a compromised tool or a
//! prompt-injection attack gets the agent's full privileges. Sandboxing
//! adds a second layer of defense so that even a compromised subprocess
//! cannot write outside the project directory or read credentials.
//!
//! # Platform support
//!
//! This first slice ships **macOS only** using `sandbox-exec` (Seatbelt).
//! Linux `bwrap` and Windows Low Integrity strategies will land as
//! follow-up PRs behind the same [`SandboxStrategy`] trait.
//!
//! # Wiring
//!
//! A [`SandboxExecutor`] is built once per session from
//! [`crate::config::schema::SandboxConfig`] and threaded into
//! [`crate::tools::ToolContext`]. Subprocess-spawning tools (currently:
//! [`Bash`](crate::tools::Bash)) call [`SandboxExecutor::wrap`] on their
//! `Command` before `.spawn()`. When sandboxing is disabled or the
//! platform has no strategy, `wrap` returns the command unchanged.

pub mod policy;
pub mod seatbelt;

pub use policy::SandboxPolicy;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::process::Command;
use tracing::{debug, warn};

use crate::config::SandboxConfig;

/// Strategy trait for wrapping a subprocess command with OS-level isolation.
pub trait SandboxStrategy: Send + Sync {
    /// Short name for diagnostics and logging (e.g. `"seatbelt"`, `"noop"`).
    fn name(&self) -> &'static str;

    /// Wrap `cmd` so the spawned child runs inside the sandbox.
    ///
    /// Implementations may build a new [`Command`] that invokes a helper
    /// (e.g. `sandbox-exec`) with the original program as its child. The
    /// returned command must preserve the original working directory and
    /// environment but does **not** need to re-apply stdio — the caller
    /// re-applies piped stdio via [`SandboxExecutor::wrap`].
    fn wrap_command(&self, cmd: Command, policy: &SandboxPolicy) -> Command;
}

/// No-op strategy used when sandboxing is disabled or unavailable.
pub struct NoopStrategy;

impl SandboxStrategy for NoopStrategy {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn wrap_command(&self, cmd: Command, _policy: &SandboxPolicy) -> Command {
        cmd
    }
}

/// Owns the active strategy and resolved policy for a session.
///
/// Construct one at session start via [`SandboxExecutor::from_config`] and
/// thread it into [`crate::tools::ToolContext::sandbox`]. Tools call
/// [`SandboxExecutor::wrap`] before spawning.
pub struct SandboxExecutor {
    strategy: Arc<dyn SandboxStrategy>,
    policy: SandboxPolicy,
    enabled: bool,
}

impl SandboxExecutor {
    /// Build an executor from config and the session's project directory.
    ///
    /// If `config.enabled` is false, the returned executor's [`wrap`] is a
    /// no-op. If the selected strategy is unavailable on the current
    /// platform (e.g. `seatbelt` on Linux), falls back to [`NoopStrategy`]
    /// and logs a warning — the caller should still treat this as enabled
    /// for `/sandbox` reporting so the degradation is visible.
    pub fn from_config(config: &SandboxConfig, project_dir: &Path) -> Self {
        let policy = SandboxPolicy::from_config(config, project_dir);
        let strategy = pick_strategy(&config.strategy);

        if config.enabled && strategy.name() == "noop" {
            warn!(
                "sandbox enabled in config but no working strategy on this platform; \
                 running without OS-level isolation"
            );
        }

        Self {
            strategy,
            policy,
            enabled: config.enabled,
        }
    }

    /// Strategy name for diagnostics (e.g. `/sandbox` command output).
    pub fn strategy_name(&self) -> &'static str {
        self.strategy.name()
    }

    /// Whether sandboxing is active (config enabled *and* a real strategy is selected).
    pub fn is_active(&self) -> bool {
        self.enabled && self.strategy.name() != "noop"
    }

    /// Access the resolved policy for diagnostics.
    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    /// Wrap `cmd` with the active strategy, reapplying piped stdio.
    ///
    /// If the executor is disabled or the strategy is a no-op, returns
    /// `cmd` unchanged. Callers should use this before `.spawn()`.
    pub fn wrap(&self, cmd: Command) -> Command {
        if !self.is_active() {
            return cmd;
        }
        debug!(
            strategy = self.strategy.name(),
            project_dir = %self.policy.project_dir.display(),
            "wrapping subprocess with sandbox"
        );
        let mut wrapped = self.strategy.wrap_command(cmd, &self.policy);
        // Strategies do not re-apply stdio (tokio hides it); force piped
        // stdio so the caller can still read stdout/stderr of the wrapped
        // child process.
        wrapped
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        wrapped
    }

    /// A "disabled" executor for tests and default construction.
    pub fn disabled() -> Self {
        Self {
            strategy: Arc::new(NoopStrategy),
            policy: SandboxPolicy {
                project_dir: PathBuf::from("."),
                allowed_write_paths: Vec::new(),
                forbidden_paths: Vec::new(),
                allow_network: true,
            },
            enabled: false,
        }
    }
}

fn pick_strategy(requested: &str) -> Arc<dyn SandboxStrategy> {
    match requested {
        "none" => Arc::new(NoopStrategy),
        "seatbelt" => make_seatbelt_or_noop(),
        "auto" | "" => auto_detect(),
        other => {
            warn!("unknown sandbox strategy {other:?}; falling back to noop");
            Arc::new(NoopStrategy)
        }
    }
}

fn auto_detect() -> Arc<dyn SandboxStrategy> {
    if cfg!(target_os = "macos") {
        make_seatbelt_or_noop()
    } else {
        Arc::new(NoopStrategy)
    }
}

fn make_seatbelt_or_noop() -> Arc<dyn SandboxStrategy> {
    if cfg!(target_os = "macos") && sandbox_exec_available() {
        Arc::new(seatbelt::SeatbeltStrategy)
    } else {
        Arc::new(NoopStrategy)
    }
}

/// True if `sandbox-exec` is resolvable on `$PATH`.
fn sandbox_exec_available() -> bool {
    // Probe via `which`-style $PATH walk. We avoid `std::process::Command`
    // here to keep this synchronous and not spawn a child at detect time.
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        if dir.join("sandbox-exec").is_file() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_strategy_none_is_noop() {
        assert_eq!(pick_strategy("none").name(), "noop");
    }

    #[test]
    fn pick_strategy_unknown_is_noop() {
        assert_eq!(pick_strategy("martian").name(), "noop");
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn auto_detect_off_macos_is_noop() {
        assert_eq!(auto_detect().name(), "noop");
    }

    #[test]
    fn disabled_executor_is_inactive() {
        let exec = SandboxExecutor::disabled();
        assert!(!exec.is_active());
        assert_eq!(exec.strategy_name(), "noop");
    }

    #[test]
    fn disabled_executor_wrap_is_identity_program() {
        // We cannot compare Commands directly, but we can verify that
        // wrapping an echo command still targets echo (not sandbox-exec)
        // when the executor is disabled.
        let exec = SandboxExecutor::disabled();
        let cmd = Command::new("echo");
        let wrapped = exec.wrap(cmd);
        let program = wrapped.as_std().get_program();
        assert_eq!(program, "echo");
    }
}
