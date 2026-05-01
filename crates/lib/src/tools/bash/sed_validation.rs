//! Refuse `sed -i` invocations whose targets the FileEdit tool would
//! reject.
//!
//! The Bash tool can express the same edit as a `sed -i ...` invocation
//! that the FileEdit tool would refuse via [`crate::permissions`]. This
//! module routes `sed -i` through that same permission gate so the two
//! surfaces stay consistent.

use std::path::{Path, PathBuf};

use crate::permissions::PermissionDecision;
use crate::tools::bash::sed_edit_parser::{SedEdit, parse_sed_edits};
use crate::tools::bash_parse::ParsedCommand;

/// Reason a `sed -i` invocation was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SedViolation {
    /// File that triggered the rejection.
    pub file: PathBuf,
    /// Underlying reason (mirrors the FileEdit denial message).
    pub reason: String,
}

/// Permission-checking interface used by [`validate_sed_edits`].
///
/// In production this is implemented by
/// [`crate::permissions::PermissionChecker`]; tests can implement it
/// directly to avoid wiring up a full config.
pub trait FileEditPermission {
    /// Return the decision that the FileEdit tool would receive for
    /// `path`. `path` is supplied as a string because the existing
    /// permission API is JSON-shaped.
    fn check_file_edit(&self, path: &str) -> PermissionDecision;
}

impl FileEditPermission for crate::permissions::PermissionChecker {
    fn check_file_edit(&self, path: &str) -> PermissionDecision {
        self.check("FileEdit", &serde_json::json!({ "file_path": path }))
    }
}

/// Validate every `sed -i` edit in `cmd` against the same permission
/// path FileEdit would use. Relative paths are resolved against `cwd`
/// before being checked.
pub fn validate_sed_edits<P: FileEditPermission>(
    cmd: &ParsedCommand,
    cwd: &Path,
    permissions: &P,
) -> Result<(), SedViolation> {
    for edit in parse_sed_edits(cmd) {
        validate_one(&edit, cwd, permissions)?;
    }
    Ok(())
}

fn validate_one<P: FileEditPermission>(
    edit: &SedEdit,
    cwd: &Path,
    permissions: &P,
) -> Result<(), SedViolation> {
    for file in &edit.files {
        let resolved = if file.is_absolute() {
            file.clone()
        } else {
            cwd.join(file)
        };
        let path_str = resolved.to_string_lossy().to_string();
        match permissions.check_file_edit(&path_str) {
            PermissionDecision::Deny(reason) => {
                return Err(SedViolation {
                    file: resolved,
                    reason,
                });
            }
            PermissionDecision::Allow | PermissionDecision::Ask(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash_parse::parse_bash;
    use std::cell::RefCell;

    /// Test double — records every check and replies with a configured
    /// decision based on a substring match against the file path.
    struct StubPermissions {
        denials: Vec<String>,
        seen: RefCell<Vec<String>>,
    }

    impl FileEditPermission for StubPermissions {
        fn check_file_edit(&self, path: &str) -> PermissionDecision {
            self.seen.borrow_mut().push(path.to_string());
            for denial in &self.denials {
                if path.contains(denial) {
                    return PermissionDecision::Deny(format!("blocked: {denial}"));
                }
            }
            PermissionDecision::Allow
        }
    }

    fn parse(s: &str) -> ParsedCommand {
        parse_bash(s).unwrap()
    }

    #[test]
    fn allows_when_target_is_not_protected() {
        let perms = StubPermissions {
            denials: vec![".git/".into()],
            seen: RefCell::new(vec![]),
        };
        let cmd = parse("sed -i 's/foo/bar/' src/lib.rs");
        let cwd = Path::new("/tmp/project");
        assert!(validate_sed_edits(&cmd, cwd, &perms).is_ok());
        assert_eq!(perms.seen.borrow().len(), 1);
    }

    #[test]
    fn blocks_when_target_is_protected_dir() {
        let perms = StubPermissions {
            denials: vec![".git/".into()],
            seen: RefCell::new(vec![]),
        };
        let cmd = parse("sed -i 's/x/y/' .git/config");
        let cwd = Path::new("/tmp/project");
        let err = validate_sed_edits(&cmd, cwd, &perms).unwrap_err();
        assert!(err.reason.contains(".git/"));
    }

    #[test]
    fn no_sed_means_no_check() {
        let perms = StubPermissions {
            denials: vec![],
            seen: RefCell::new(vec![]),
        };
        let cmd = parse("ls -la");
        assert!(validate_sed_edits(&cmd, Path::new("/tmp"), &perms).is_ok());
        assert!(perms.seen.borrow().is_empty());
    }

    #[test]
    fn sed_without_inplace_is_skipped() {
        let perms = StubPermissions {
            denials: vec![".git/".into()],
            seen: RefCell::new(vec![]),
        };
        let cmd = parse("sed 's/x/y/' .git/config");
        assert!(validate_sed_edits(&cmd, Path::new("/tmp"), &perms).is_ok());
    }

    #[test]
    fn relative_paths_are_resolved_against_cwd() {
        let perms = StubPermissions {
            denials: vec!["/tmp/project/.git".into()],
            seen: RefCell::new(vec![]),
        };
        let cmd = parse("sed -i 's/x/y/' .git/HEAD");
        let cwd = Path::new("/tmp/project");
        let err = validate_sed_edits(&cmd, cwd, &perms).unwrap_err();
        assert_eq!(err.file, PathBuf::from("/tmp/project/.git/HEAD"));
    }

    #[test]
    fn multiple_files_all_checked() {
        let perms = StubPermissions {
            denials: vec!["node_modules/".into()],
            seen: RefCell::new(vec![]),
        };
        let cmd = parse("sed -i 's/x/y/' a.txt node_modules/pkg/index.js");
        let err = validate_sed_edits(&cmd, Path::new("/repo"), &perms).unwrap_err();
        assert!(err.file.to_string_lossy().contains("node_modules/"));
    }
}
