//! Decide how a single Bash tool call should be executed: sandboxed,
//! prompted to the user, or run freely.
//!
//! This module pulls together the destructive-pattern classifier and
//! the coarse effect classifier so [`crate::tools::bash::BashTool`] has
//! exactly one place that says "for this command, here is the decision".

use crate::tools::bash::bash_security::{DestructivenessLevel, classify_destructive};
use crate::tools::bash::command_semantics::{Effect, classify};
use crate::tools::bash_parse::ParsedCommand;

/// What the runtime should do with a given command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxDecision {
    /// Run inside the configured sandbox wrapper.
    Sandboxed,
    /// Prompt the user before running.
    Prompt(String),
    /// Refuse outright; the message describes why.
    Refuse(String),
    /// Run without any sandbox wrapping (read-only commands).
    RunFreely,
}

/// Inputs the decision function needs.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Whether the configured sandbox is currently usable.
    pub sandbox_available: bool,
    /// Whether the caller passed `dangerouslyDisableSandbox: true`.
    pub dangerously_disable_sandbox: bool,
    /// Whether the session policy permits a sandbox bypass.
    pub allow_bypass: bool,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            sandbox_available: false,
            dangerously_disable_sandbox: false,
            allow_bypass: true,
        }
    }
}

/// Decide what to do with `cmd` given the execution context.
///
/// The decision flow is:
///
/// 1. Destructive validation runs unconditionally. The
///    `dangerouslyDisableSandbox` flag does not weaken this gate; it
///    only chooses between "sandbox-wrapped" and "raw-host" execution
///    once the command has been judged safe.
/// 2. If the destructive-command classifier flags the command, refuse.
/// 3. If `dangerouslyDisableSandbox` is set and the policy allows
///    bypass, run freely (the model has explicit clearance).
/// 4. If a sandbox is available, sandbox the call.
/// 5. If the command is read-only, run freely.
/// 6. Otherwise prompt.
pub fn decide(cmd: &ParsedCommand, ctx: &ExecutionContext) -> SandboxDecision {
    // Destructive-pattern check always runs. Skipping it on the
    // `dangerouslyDisableSandbox` path historically allowed `rm -rf /`
    // to bypass validation entirely.
    match classify_destructive(cmd) {
        DestructivenessLevel::Destructive => {
            return SandboxDecision::Refuse(
                "Command flagged as destructive; refuse before exec.".into(),
            );
        }
        DestructivenessLevel::Risky => {
            return SandboxDecision::Prompt("Command is risky; ask the user to confirm.".into());
        }
        DestructivenessLevel::Safe => {}
    }

    if ctx.dangerously_disable_sandbox && ctx.allow_bypass {
        return SandboxDecision::RunFreely;
    }

    if ctx.sandbox_available {
        return SandboxDecision::Sandboxed;
    }

    let effects = classify(cmd);
    if !effects.is_empty() && effects.iter().all(|e| matches!(e, Effect::ReadOnly)) {
        return SandboxDecision::RunFreely;
    }

    SandboxDecision::Prompt(
        "No sandbox available and command may mutate state; ask the user to confirm.".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash_parse::parse_bash;

    fn parse(s: &str) -> ParsedCommand {
        let mut p = parse_bash(s).unwrap_or_default();
        p.raw = s.to_string();
        p
    }

    fn ctx(sandbox: bool) -> ExecutionContext {
        ExecutionContext {
            sandbox_available: sandbox,
            dangerously_disable_sandbox: false,
            allow_bypass: true,
        }
    }

    #[test]
    fn sandbox_available_means_sandboxed() {
        let cmd = parse("ls -la");
        assert_eq!(decide(&cmd, &ctx(true)), SandboxDecision::Sandboxed);
    }

    #[test]
    fn read_only_runs_freely_without_sandbox() {
        let cmd = parse("ls -la");
        assert_eq!(decide(&cmd, &ctx(false)), SandboxDecision::RunFreely);
    }

    #[test]
    fn destructive_is_refused_even_with_sandbox() {
        let cmd = parse("rm -rf /tmp/foo");
        let decision = decide(&cmd, &ctx(true));
        assert!(matches!(decision, SandboxDecision::Refuse(_)));
    }

    #[test]
    fn dangerously_disable_runs_freely() {
        let cmd = parse("ls");
        let mut c = ctx(true);
        c.dangerously_disable_sandbox = true;
        assert_eq!(decide(&cmd, &c), SandboxDecision::RunFreely);
    }

    #[test]
    fn dangerously_disable_ignored_when_bypass_blocked() {
        let cmd = parse("ls");
        let mut c = ctx(true);
        c.dangerously_disable_sandbox = true;
        c.allow_bypass = false;
        assert_eq!(decide(&cmd, &c), SandboxDecision::Sandboxed);
    }

    #[test]
    fn mutating_without_sandbox_prompts() {
        let cmd = parse("cp src/a.txt dest/a.txt");
        let decision = decide(&cmd, &ctx(false));
        assert!(matches!(decision, SandboxDecision::Prompt(_)));
    }

    #[test]
    fn destructive_outranks_sandbox_decision() {
        let cmd = parse("git push --force origin main");
        let decision = decide(&cmd, &ctx(true));
        assert!(matches!(decision, SandboxDecision::Refuse(_)));
    }

    #[test]
    fn dangerously_disable_does_not_skip_destructive_check() {
        // `dangerouslyDisableSandbox` must NOT bypass the destructive
        // validator. Skipping the check here would let `rm -rf /` slip
        // through the bypass path historically.
        let cmd = parse("rm -rf /");
        let mut c = ctx(true);
        c.dangerously_disable_sandbox = true;
        c.allow_bypass = true;
        let decision = decide(&cmd, &c);
        assert!(
            matches!(decision, SandboxDecision::Refuse(_)),
            "expected Refuse, got {decision:?}"
        );
    }
}
