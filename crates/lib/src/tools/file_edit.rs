//! FileEdit tool: targeted search-and-replace editing.
//!
//! Performs exact string replacement within a file. The `old_string`
//! must match uniquely (unless `replace_all` is set) to prevent
//! ambiguous edits.
//!
//! Before writing, the tool checks whether the file's modification time
//! has changed since it was last read (via the session file cache). If
//! the file is stale the edit is rejected so the model re-reads first.
//!
//! After a successful edit the tool returns a unified diff of the
//! changes so the model (and user) can see exactly what happened.

use async_trait::async_trait;
use serde_json::json;
use similar::TextDiff;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub struct FileEditTool;

/// Check whether the file on disk was modified after the cache recorded it.
///
/// Returns `Ok(())` when:
///   - the cache holds an entry and the on-disk mtime still matches, or
///   - there is no cache / no entry (we cannot prove staleness).
///
/// Returns an error message when the mtimes diverge.
async fn check_staleness(path: &Path, ctx: &ToolContext) -> Result<(), String> {
    let cache = match ctx.file_cache.as_ref() {
        Some(c) => c,
        None => return Ok(()),
    };

    let cached_mtime: SystemTime = {
        let guard = cache.lock().await;
        match guard.last_read_mtime(path) {
            Some(t) => t,
            None => return Ok(()), // never read through cache — nothing to compare
        }
    };

    let disk_mtime = tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|m| m.modified().ok());

    if let Some(disk) = disk_mtime
        && disk != cached_mtime
    {
        return Err(format!(
            "File was modified since last read. \
             Please re-read {} before editing.",
            path.display()
        ));
    }

    Ok(())
}

/// Produce a compact unified diff between `old` and `new` text.
///
/// The output uses `---`/`+++` headers with the file path and includes
/// only the changed hunks with a few lines of context.
fn unified_diff(file_path: &str, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();

    // Header
    out.push_str(&format!("--- {file_path}\n"));
    out.push_str(&format!("+++ {file_path}\n"));

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        out.push_str(&format!("{hunk}"));
    }

    // If the diff is empty (shouldn't happen because we already verified
    // old_string != new_string), fall back to a note.
    if out.lines().count() <= 2 {
        out.push_str("(no visible diff)\n");
    }

    out
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &'static str {
        "FileEdit"
    }

    fn description(&self) -> &'static str {
        "Performs exact string replacements in files. The old_string must \
         match uniquely unless replace_all is true."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["file_path", "old_string", "new_string"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text (must differ from old_string)"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)",
                    "default": false
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn get_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'file_path' is required".into()))?;

        let old_string = input
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'old_string' is required".into()))?;

        let new_string = input
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'new_string' is required".into()))?;

        let replace_all = input
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if old_string == new_string {
            return Err(ToolError::InvalidInput(
                "old_string and new_string must be different".into(),
            ));
        }

        let path = Path::new(file_path);

        // Check file size before reading (reject files > 1MB).
        const MAX_EDIT_SIZE: u64 = 1_048_576;
        if let Ok(meta) = tokio::fs::metadata(file_path).await
            && meta.len() > MAX_EDIT_SIZE
        {
            return Err(ToolError::InvalidInput(format!(
                "File too large for editing ({} bytes, max {}). \
                 Consider using Bash with sed/awk for large files.",
                meta.len(),
                MAX_EDIT_SIZE
            )));
        }

        // Staleness check: reject if the file changed since the model last
        // read it, so the model works with up-to-date content.
        if let Err(msg) = check_staleness(path, ctx).await {
            return Err(ToolError::ExecutionFailed(msg));
        }

        let content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read {file_path}: {e}")))?;

        let occurrences = content.matches(old_string).count();

        if occurrences == 0 {
            return Err(ToolError::InvalidInput(format!(
                "old_string not found in {file_path}"
            )));
        }

        if occurrences > 1 && !replace_all {
            return Err(ToolError::InvalidInput(format!(
                "old_string has {occurrences} occurrences in {file_path}. \
                 Use replace_all=true to replace all, or provide a more \
                 specific old_string."
            )));
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        tokio::fs::write(file_path, &new_content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write {file_path}: {e}")))?;

        // Invalidate the cache entry so the next read picks up our write.
        if let Some(cache) = ctx.file_cache.as_ref() {
            let mut guard = cache.lock().await;
            guard.invalidate(path);
        }

        // Build a unified diff so the model/user sees exactly what changed.
        let replaced = if replace_all { occurrences } else { 1 };
        let diff = unified_diff(file_path, &content, &new_content);
        Ok(ToolResult::success(format!(
            "Replaced {replaced} occurrence(s) in {file_path}\n\n{diff}"
        )))
    }
}
