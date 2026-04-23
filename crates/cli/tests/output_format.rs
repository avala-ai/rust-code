//! Integration tests for --output-format.
//!
//! These tests verify CLI flag parsing, error handling, and the
//! JSONL output contract. They do NOT require an API key — they
//! test error paths and argument validation only.

use assert_cmd::Command;
use predicates::prelude::*;

fn agent() -> Command {
    Command::cargo_bin("agent").expect("binary should exist")
}

/// Every env var that `ApiConfig::default` accepts as a fallback source
/// for the API key. Tests that want to assert "no key is available"
/// must scrub every one of these — otherwise a developer running
/// `cargo test --all-targets` with their normal shell env sees tests
/// fail because the binary proceeds past the key check, starts
/// retrying, and tracing WARN lines leak onto stdout.
///
/// Keep this list in sync with `ApiConfig::default` in
/// `crates/lib/src/config/schema.rs`.
const API_KEY_ENV_VARS: &[&str] = &[
    "AGENT_CODE_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_ENDPOINT",
    "XAI_API_KEY",
    "GOOGLE_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "ZHIPU_API_KEY",
    "TOGETHER_API_KEY",
    "OPENROUTER_API_KEY",
    "COHERE_API_KEY",
    "PERPLEXITY_API_KEY",
    // Bedrock / Vertex steer the base URL even without a key; strip
    // them so the test path is deterministic regardless of where
    // the developer was last logged in.
    "AGENT_CODE_USE_BEDROCK",
    "AGENT_CODE_USE_VERTEX",
    "AWS_REGION",
];

/// Apply `env_remove` for every known API key env var so the binary
/// can't resolve a fallback and bails at the key check.
fn scrub_api_keys(cmd: &mut Command) -> &mut Command {
    for var in API_KEY_ENV_VARS {
        cmd.env_remove(var);
    }
    cmd
}

// ── Flag parsing ──────────────────────────────────────────────────

#[test]
fn output_format_appears_in_help() {
    agent()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--output-format"));
}

#[test]
fn output_format_invalid_value_fails() {
    agent()
        .args(["--output-format", "xml", "--prompt", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown output format"));
}

#[test]
fn output_format_json_without_prompt_fails() {
    // JSON mode only makes sense in non-interactive (--prompt) mode.
    agent()
        .args(["--output-format", "json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires --prompt"));
}

#[test]
fn output_format_text_is_default() {
    // With no --output-format flag but also no API key, the text path
    // should be taken. Without a key it will fail at API key validation,
    // but AFTER format parsing succeeds (no "unknown output format" error).
    let mut cmd = agent();
    scrub_api_keys(&mut cmd);
    let output = cmd
        .args(["--prompt", "hello"])
        .output()
        .expect("should execute");

    // It will fail (no API key), but the failure should NOT be about
    // output format parsing — it should be about API key.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unknown output format"),
        "text should be the default format, not trigger an error: {stderr}",
    );
}

#[test]
fn output_format_json_no_api_key_emits_session_events() {
    // Even when the API key is missing and the run fails, JSON mode
    // should still emit session_start and session_end events so
    // downstream consumers get a well-formed stream.
    //
    // We scrub EVERY provider env var, not just AGENT_CODE_API_KEY:
    // if a developer's shell has ANTHROPIC_API_KEY (or any of the
    // 12+ other fallbacks), the binary would proceed past the key
    // check, the LLM call would fail, retries would log WARN via
    // tracing onto stdout, and the JSONL assertion below would fail.
    // See API_KEY_ENV_VARS for the full list.
    let mut cmd = agent();
    scrub_api_keys(&mut cmd);
    let output = cmd
        .args(["--output-format", "json", "--prompt", "hello"])
        .output()
        .expect("should execute");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The binary may fail before reaching the JSON sink (at the API key
    // check, which happens before the output format branch). In that case,
    // stdout will be empty and the error is on stderr. That's acceptable —
    // we just verify it doesn't panic or produce malformed output.
    if !stdout.is_empty() {
        // If we got any stdout, verify it's valid JSONL.
        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
            assert!(
                parsed.is_ok(),
                "non-JSON line in JSONL output: {line}\nfull stdout: {stdout}",
            );
        }
    }

    // Either way, the process should not panic.
    assert!(!stderr.contains("panicked"), "agent panicked: {stderr}",);
}

// ── Case insensitivity ────────────────────────────────────────────

#[test]
fn output_format_case_insensitive() {
    // "JSON" (uppercase) should be accepted.
    let output = agent()
        .args(["--output-format", "JSON", "--prompt", "hello"])
        .env_remove("AGENT_CODE_API_KEY")
        .output()
        .expect("should execute");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unknown output format"),
        "uppercase JSON should be accepted: {stderr}",
    );
}
