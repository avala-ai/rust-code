//! Coarse classification of bash commands by side effect.
//!
//! Given a [`ParsedCommand`], return the set of [`Effect`]s that any of
//! its component commands trigger. A single shell pipeline can have
//! multiple effects (for example `curl url | tee file` is both
//! [`Effect::Network`] and [`Effect::Mutating`]).
//!
//! This module is intentionally conservative: when a command name is
//! not recognised it is treated as [`Effect::Mutating`] so callers err
//! on the side of caution.

use std::collections::BTreeSet;

use crate::tools::bash_parse::ParsedCommand;

/// Coarse-grained effect categories used by the safety pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Effect {
    /// Reads state without mutating anything (cat, ls, grep, ...).
    ReadOnly,
    /// Mutates the filesystem or other local state.
    Mutating,
    /// Touches the network (curl, wget, ssh, ...).
    Network,
    /// Requires elevated privileges (sudo, su, doas).
    Privileged,
}

/// Compute the union of effects for every command in `parsed`.
pub fn classify(parsed: &ParsedCommand) -> Vec<Effect> {
    let mut set: BTreeSet<Effect> = BTreeSet::new();

    if parsed.commands.is_empty() {
        return Vec::new();
    }

    for raw in &parsed.commands {
        for effect in classify_single(raw) {
            set.insert(effect);
        }
    }

    set.into_iter().collect()
}

/// Convenience helper: true if [`Effect::ReadOnly`] is the only effect.
pub fn is_read_only(parsed: &ParsedCommand) -> bool {
    let effects = classify(parsed);
    !effects.is_empty() && effects.iter().all(|e| *e == Effect::ReadOnly)
}

/// Classify a single command name (path components stripped).
pub fn classify_single(raw_name: &str) -> Vec<Effect> {
    let base = base_command(raw_name);

    let mut effects = Vec::new();

    if PRIVILEGED.contains(&base) {
        effects.push(Effect::Privileged);
    }
    if NETWORK.contains(&base) {
        effects.push(Effect::Network);
    }
    if FILE_WRITERS.contains(&base) {
        effects.push(Effect::Mutating);
    }
    if FILE_READERS.contains(&base) {
        effects.push(Effect::ReadOnly);
    }

    if effects.is_empty() {
        // Unknown commands are conservatively treated as mutating; this
        // means the safety pipeline cannot be silently bypassed by an
        // exotic binary name.
        effects.push(Effect::Mutating);
    }

    effects
}

/// Strip a leading path and any leading quote characters from a raw
/// command name extracted by the bash parser.
fn base_command(raw: &str) -> &str {
    let trimmed = raw.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Commands that read filesystem state without mutating it.
const FILE_READERS: &[&str] = &[
    "cat",
    "less",
    "more",
    "head",
    "tail",
    "ls",
    "ll",
    "find",
    "grep",
    "rg",
    "egrep",
    "fgrep",
    "du",
    "df",
    "stat",
    "file",
    "wc",
    "sort",
    "uniq",
    "awk",
    "cut",
    "tr",
    "echo",
    "printf",
    "true",
    "false",
    "pwd",
    "which",
    "whereis",
    "type",
    "id",
    "whoami",
    "date",
    "uname",
    "hostname",
    "env",
    "printenv",
    "ps",
    "top",
    "uptime",
    "free",
    "vmstat",
    "lsof",
    "ip",
    "ifconfig",
    "netstat",
    "ss",
    "dig",
    "host",
    "nslookup",
    "git",
    "diff",
    "cmp",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "basename",
    "dirname",
    "readlink",
    "realpath",
    "test",
    "[",
];

/// Commands that mutate filesystem or other local state.
const FILE_WRITERS: &[&str] = &[
    "cp", "mv", "rm", "rmdir", "mkdir", "touch", "chmod", "chown", "chgrp", "ln", "tee", "dd",
    "shred", "truncate", "install", "patch", "sed", "tar", "gzip", "gunzip", "zip", "unzip", "xz",
    "bzip2", "bunzip2",
];

/// Commands that touch the network.
const NETWORK: &[&str] = &[
    "curl",
    "wget",
    "nc",
    "ncat",
    "ssh",
    "scp",
    "rsync",
    "sftp",
    "ftp",
    "git-remote-https",
    "telnet",
];

/// Commands that require elevated privileges.
const PRIVILEGED: &[&str] = &["sudo", "su", "doas", "pkexec"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash_parse::parse_bash;

    fn classify_str(cmd: &str) -> Vec<Effect> {
        let parsed = parse_bash(cmd).expect("parse");
        classify(&parsed)
    }

    #[test]
    fn read_only_commands_are_read_only() {
        assert_eq!(classify_str("ls -la"), vec![Effect::ReadOnly]);
        assert_eq!(classify_str("grep TODO src/lib.rs"), vec![Effect::ReadOnly]);
        assert_eq!(classify_str("cat README.md"), vec![Effect::ReadOnly]);
    }

    #[test]
    fn file_writers_are_mutating() {
        assert!(classify_str("cp a b").contains(&Effect::Mutating));
        assert!(classify_str("rm -rf dir").contains(&Effect::Mutating));
        assert!(classify_str("mkdir foo").contains(&Effect::Mutating));
    }

    #[test]
    fn network_commands_flagged() {
        assert!(classify_str("curl https://example.com").contains(&Effect::Network));
        assert!(classify_str("wget http://x").contains(&Effect::Network));
        assert!(classify_str("ssh host").contains(&Effect::Network));
    }

    #[test]
    fn privileged_commands_flagged() {
        assert!(classify_str("sudo apt install foo").contains(&Effect::Privileged));
        assert!(classify_str("doas reboot").contains(&Effect::Privileged));
    }

    #[test]
    fn pipeline_unions_effects() {
        let effects = classify_str("curl https://example.com | tee out.txt");
        assert!(effects.contains(&Effect::Network));
        assert!(effects.contains(&Effect::Mutating));
    }

    #[test]
    fn unknown_command_is_mutating() {
        let effects = classify_single("some-unknown-binary");
        assert_eq!(effects, vec![Effect::Mutating]);
    }

    #[test]
    fn is_read_only_helper() {
        let parsed = parse_bash("ls -la").unwrap();
        assert!(is_read_only(&parsed));
        let parsed = parse_bash("rm foo").unwrap();
        assert!(!is_read_only(&parsed));
        let parsed = parse_bash("cat a | grep b").unwrap();
        assert!(is_read_only(&parsed));
    }

    #[test]
    fn empty_command_has_no_effects() {
        let parsed = ParsedCommand::default();
        assert!(classify(&parsed).is_empty());
    }
}
