//! End-to-end test for output-style propagation across the
//! Agent-tool subprocess boundary.
//!
//! Regression for the codex finding that `applies_to: [subagent]`
//! styles never reached the spawned child: the parent set
//! `AGENT_CODE_SUBAGENT=1` but did not propagate the active
//! disk-loaded style. The fix introduces
//! `AGENT_CODE_DISK_OUTPUT_STYLE=<name>`; this test boots the binary
//! with that env var and `--dump-system-prompt` and asserts the
//! prompt contains the disk style's body.

use assert_cmd::Command;
use std::fs;

const STYLE_BODY: &str =
    "Be a marker phrase that should not occur in the default prompt: subagent_propagation_works.";

fn agent() -> Command {
    Command::cargo_bin("agent").expect("binary should exist")
}

#[test]
fn subagent_inherits_disk_output_style_via_env_var() {
    let project = tempfile::tempdir().expect("tempdir");
    let styles_dir = project.path().join(".agent").join("output-styles");
    fs::create_dir_all(&styles_dir).unwrap();
    let style_path = styles_dir.join("subagent-only.md");
    fs::write(
        &style_path,
        format!(
            "---\n\
             name: subagent-only\n\
             description: subagent-only style fixture\n\
             applies_to:\n  - subagent\n\
             ---\n\
             {STYLE_BODY}",
        ),
    )
    .unwrap();

    // Pin the user-level dir to an empty tempdir so this test cannot
    // pick up whatever may live in `~/.config/agent-code/output-styles/`
    // on the developer's machine.
    let user_dir = tempfile::tempdir().expect("user tempdir");

    let output = agent()
        .arg("--dump-system-prompt")
        .arg("--cwd")
        .arg(project.path())
        .env("AGENT_CODE_SUBAGENT", "1")
        .env("AGENT_CODE_DISK_OUTPUT_STYLE", "subagent-only")
        .env("AGENT_CODE_USER_OUTPUT_STYLES_DIR", user_dir.path())
        // No API key needed for --dump-system-prompt, but scrub anyway
        // so a developer's environment can't influence the prompt.
        .env_remove("AGENT_CODE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("failed to invoke binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "binary should exit cleanly under --dump-system-prompt; stderr: {stderr}"
    );
    assert!(
        stdout.contains(STYLE_BODY),
        "subagent-applied disk style body must appear in the dumped system prompt; \
         stdout was:\n{stdout}",
    );
}

#[test]
fn missing_disk_output_style_logs_warning_and_continues() {
    // The CLI must NOT crash when AGENT_CODE_DISK_OUTPUT_STYLE points
    // at a style that doesn't exist (e.g. parent had a project file
    // but the subagent runs in a worktree without it).
    let project = tempfile::tempdir().expect("tempdir");
    let user_dir = tempfile::tempdir().expect("user tempdir");

    let output = agent()
        .arg("--dump-system-prompt")
        .arg("--cwd")
        .arg(project.path())
        .env("AGENT_CODE_SUBAGENT", "1")
        .env("AGENT_CODE_DISK_OUTPUT_STYLE", "no-such-style")
        .env("AGENT_CODE_USER_OUTPUT_STYLES_DIR", user_dir.path())
        .env_remove("AGENT_CODE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("failed to invoke binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "binary must not crash on a missing style id; stderr: {stderr}"
    );
}
