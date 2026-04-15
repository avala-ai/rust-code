//! Integration tests for process-level sandboxing.
//!
//! The real end-to-end test is macOS-only because Seatbelt is the only
//! shipping strategy. On other platforms we still run a smoke test against
//! the disabled executor to guard the pass-through path.

use std::sync::Arc;

use agent_code_lib::config::SandboxConfig;
use agent_code_lib::permissions::PermissionChecker;
use agent_code_lib::sandbox::SandboxExecutor;
use agent_code_lib::tools::bash::BashTool;
use agent_code_lib::tools::{Tool, ToolContext};
use tokio_util::sync::CancellationToken;

fn make_ctx(cwd: std::path::PathBuf, sandbox: Option<Arc<SandboxExecutor>>) -> ToolContext {
    ToolContext {
        cwd,
        cancel: CancellationToken::new(),
        permission_checker: Arc::new(PermissionChecker::allow_all()),
        verbose: false,
        plan_mode: false,
        file_cache: None,
        denial_tracker: None,
        task_manager: None,
        session_allows: None,
        permission_prompter: None,
        sandbox,
    }
}

#[tokio::test]
async fn bash_runs_normally_with_disabled_sandbox() {
    // Regression guard: wrapping with a disabled sandbox must not change
    // command behavior — `echo hello` still returns `hello` on stdout.
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(
        tmp.path().to_path_buf(),
        Some(Arc::new(SandboxExecutor::disabled())),
    );
    let bash = BashTool;
    let result = bash
        .call(serde_json::json!({"command": "echo hello"}), &ctx)
        .await
        .expect("bash echo hello should succeed");
    assert!(
        result.content.contains("hello"),
        "expected stdout to contain 'hello', got: {}",
        result.content
    );
    assert!(!result.is_error);
}

#[tokio::test]
async fn bash_runs_normally_without_sandbox_context() {
    // Second guard for the no-sandbox-threaded path (ctx.sandbox = None).
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path().to_path_buf(), None);
    let bash = BashTool;
    let result = bash
        .call(serde_json::json!({"command": "echo goodbye"}), &ctx)
        .await
        .expect("bash echo goodbye should succeed");
    assert!(result.content.contains("goodbye"));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn seatbelt_blocks_writes_outside_project_dir() {
    // End-to-end: enable sandbox, try to write to /etc, expect failure.
    let tmp = tempfile::tempdir().unwrap();
    let cfg = SandboxConfig {
        enabled: true,
        strategy: "seatbelt".to_string(),
        allowed_write_paths: vec![],
        forbidden_paths: vec![],
        allow_network: false,
    };
    let exec = Arc::new(SandboxExecutor::from_config(&cfg, tmp.path()));

    // Skip if seatbelt is unavailable (e.g. a minimal macOS CI image).
    if !exec.is_active() {
        eprintln!("skipping: seatbelt not available on this runner");
        return;
    }

    let ctx = make_ctx(tmp.path().to_path_buf(), Some(exec));
    let bash = BashTool;
    let marker = "/etc/agent-code-sandbox-test-marker";
    let result = bash
        .call(
            serde_json::json!({
                "command": format!("echo test > {marker}"),
            }),
            &ctx,
        )
        .await
        .expect("bash tool call should return a ToolResult even on sandbox denial");

    assert!(
        result.is_error
            || result.content.contains("Operation not permitted")
            || result.content.contains("sandbox")
            || result.content.contains("denied"),
        "expected sandbox denial, got: {}",
        result.content
    );

    // Defense in depth: confirm the marker file was not actually created.
    assert!(
        !std::path::Path::new(marker).exists(),
        "sandbox should have blocked creation of {marker}"
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn seatbelt_allows_writes_inside_project_dir() {
    // Regression guard: writes inside the project directory must still
    // succeed when the sandbox is active, otherwise the sandbox is useless.
    let tmp = tempfile::tempdir().unwrap();
    let cfg = SandboxConfig {
        enabled: true,
        strategy: "seatbelt".to_string(),
        allowed_write_paths: vec![],
        forbidden_paths: vec![],
        allow_network: false,
    };
    let exec = Arc::new(SandboxExecutor::from_config(&cfg, tmp.path()));

    if !exec.is_active() {
        eprintln!("skipping: seatbelt not available on this runner");
        return;
    }

    let ctx = make_ctx(tmp.path().to_path_buf(), Some(exec));
    let bash = BashTool;
    let result = bash
        .call(
            serde_json::json!({
                "command": "echo sandboxed > inside.txt && cat inside.txt",
            }),
            &ctx,
        )
        .await
        .expect("bash tool call should succeed");
    assert!(
        !result.is_error,
        "inside-project write failed: {}",
        result.content
    );
    assert!(result.content.contains("sandboxed"));
}
