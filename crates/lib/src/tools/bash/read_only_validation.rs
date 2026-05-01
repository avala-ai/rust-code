//! Refuse mutating bash commands when the active permission profile
//! demands read-only operation.
//!
//! Used by [`crate::tools::bash::BashTool`] when running under a
//! permission profile (e.g. plan mode) that should never let a shell
//! pipeline mutate state. The check runs after `bash_parse` has built a
//! [`ParsedCommand`] but before the command is dispatched.

use crate::config::PermissionMode;
use crate::tools::bash::command_semantics::{Effect, classify};
use crate::tools::bash_parse::ParsedCommand;

/// A coarse summary of what the active permission policy permits.
///
/// Wraps [`PermissionMode`] so callers do not have to reason about the
/// individual config-level variants. `Allow` and `AcceptEdits` map to
/// [`PermissionProfile::Permissive`]; `Plan` and `Deny` map to
/// [`PermissionProfile::ReadOnly`]; `Ask` maps to
/// [`PermissionProfile::Prompted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionProfile {
    /// Anything goes (subject to the regular destructive-pattern checks).
    Permissive,
    /// Tool calls are allowed but require user confirmation.
    Prompted,
    /// Only read-only commands may run; mutating/network/privileged
    /// commands must be refused before exec.
    ReadOnly,
}

impl From<PermissionMode> for PermissionProfile {
    fn from(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::Allow | PermissionMode::AcceptEdits => PermissionProfile::Permissive,
            PermissionMode::Ask => PermissionProfile::Prompted,
            PermissionMode::Deny | PermissionMode::Plan => PermissionProfile::ReadOnly,
        }
    }
}

/// Reason a command was rejected as not-read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlyViolation {
    /// Effects that triggered the rejection.
    pub effects: Vec<Effect>,
    /// User-facing message.
    pub message: String,
}

/// Decide whether `cmd` may run under `profile`.
///
/// Returns `Ok(())` when the command is allowed under the profile.
/// Returns `Err` when the profile is read-only and the command would
/// mutate state, touch the network, or escalate privileges.
pub fn validate_read_only(
    cmd: &ParsedCommand,
    profile: &PermissionProfile,
) -> Result<(), ReadOnlyViolation> {
    if !matches!(profile, PermissionProfile::ReadOnly) {
        return Ok(());
    }

    let effects = classify(cmd);
    let blocking: Vec<Effect> = effects
        .iter()
        .copied()
        .filter(|e| !matches!(e, Effect::ReadOnly))
        .collect();

    if blocking.is_empty() {
        return Ok(());
    }

    let names: Vec<String> = blocking.iter().map(|e| format!("{e:?}")).collect();
    Err(ReadOnlyViolation {
        effects: blocking,
        message: format!(
            "Read-only profile refuses command with effects: {}. \
             Switch to a more permissive profile to run mutating commands.",
            names.join(", ")
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash_parse::parse_bash;

    fn parse(s: &str) -> ParsedCommand {
        parse_bash(s).unwrap()
    }

    #[test]
    fn permissive_profile_allows_everything() {
        let cmd = parse("rm -rf /tmp/foo");
        assert!(validate_read_only(&cmd, &PermissionProfile::Permissive).is_ok());
    }

    #[test]
    fn prompted_profile_allows_everything_at_validation_layer() {
        // The prompt happens elsewhere; this layer only blocks read-only.
        let cmd = parse("rm -rf /tmp/foo");
        assert!(validate_read_only(&cmd, &PermissionProfile::Prompted).is_ok());
    }

    #[test]
    fn read_only_profile_allows_reads() {
        let cmd = parse("ls -la");
        assert!(validate_read_only(&cmd, &PermissionProfile::ReadOnly).is_ok());
    }

    #[test]
    fn read_only_profile_blocks_writes() {
        let cmd = parse("rm foo");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }

    #[test]
    fn read_only_profile_blocks_network() {
        let cmd = parse("curl https://example.com");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Network));
    }

    #[test]
    fn read_only_profile_blocks_privileged() {
        let cmd = parse("sudo ls");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Privileged));
    }

    #[test]
    fn from_permission_mode_maps_correctly() {
        assert_eq!(
            PermissionProfile::from(PermissionMode::Allow),
            PermissionProfile::Permissive
        );
        assert_eq!(
            PermissionProfile::from(PermissionMode::AcceptEdits),
            PermissionProfile::Permissive
        );
        assert_eq!(
            PermissionProfile::from(PermissionMode::Ask),
            PermissionProfile::Prompted
        );
        assert_eq!(
            PermissionProfile::from(PermissionMode::Plan),
            PermissionProfile::ReadOnly
        );
        assert_eq!(
            PermissionProfile::from(PermissionMode::Deny),
            PermissionProfile::ReadOnly
        );
    }

    #[test]
    fn read_only_profile_rejects_chained_writes() {
        let cmd = parse("ls && rm foo");
        let err = validate_read_only(&cmd, &PermissionProfile::ReadOnly).unwrap_err();
        assert!(err.effects.contains(&Effect::Mutating));
    }
}
