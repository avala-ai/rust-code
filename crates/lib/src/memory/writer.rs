//! Memory write discipline.
//!
//! Enforces the two-step write pattern:
//! 1. Write the memory file with proper frontmatter
//! 2. Update MEMORY.md index with a one-line pointer
//!
//! Prevents entropy by never dumping content into the index.

use std::path::{Path, PathBuf};

use super::types::{MemoryMeta, MemoryType};

/// Maximum index line length.
const MAX_INDEX_LINE_CHARS: usize = 150;

/// Maximum index lines before truncation.
const MAX_INDEX_LINES: usize = 200;

/// Maximum allowed length for a memory filename (including the `.md` suffix).
const MAX_FILENAME_LEN: usize = 128;

/// Reserved index filename. Rejected case-insensitively so the slug
/// `memory.md` cannot collide with `MEMORY.md` on case-folding
/// filesystems (default macOS, Windows).
const RESERVED_INDEX_NAMES: &[&str] = &["memory.md", "readme.md"];

/// Windows-reserved device names. Refused as bare stems
/// case-insensitively (with or without an extension) so a slug like
/// `con.md` cannot land on a Windows checkout.
const WINDOWS_RESERVED_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Validate a memory filename. Rejects anything that could escape the
/// memory directory or smuggle control characters into the index.
///
/// Rules:
/// - Non-empty.
/// - At most `MAX_FILENAME_LEN` bytes (and not `.` / `..`).
/// - No path separators (`/`, `\`).
/// - No `..` segments (rejects `..`, `foo/..`, etc.).
/// - No NUL, newline, carriage return, or other ASCII control characters.
/// - ASCII-printable only — keeps cross-platform behavior predictable.
/// - No trailing `.` or whitespace (Windows strips these silently,
///   creating two filenames that look distinct but resolve to the same
///   file).
/// - Case-insensitive reject of the index file (`MEMORY.md`) and
///   `README.md` to avoid silent collisions on case-folding
///   filesystems.
/// - Case-insensitive reject of Windows-reserved device names
///   (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`),
///   with or without an extension.
fn validate_memory_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty() {
        return Err("memory filename must not be empty".into());
    }
    if filename.len() > MAX_FILENAME_LEN {
        return Err(format!(
            "memory filename too long ({} > {MAX_FILENAME_LEN} bytes)",
            filename.len()
        ));
    }
    if filename == "." || filename == ".." {
        return Err(format!("memory filename '{filename}' is not allowed"));
    }
    for ch in filename.chars() {
        if ch == '/' || ch == '\\' {
            return Err(format!(
                "memory filename '{filename}' must not contain path separators"
            ));
        }
        if ch == '\0' {
            return Err("memory filename must not contain NUL".into());
        }
        if ch.is_control() {
            return Err(format!(
                "memory filename '{filename}' must not contain control characters"
            ));
        }
        if !ch.is_ascii() || !ch.is_ascii_graphic() {
            return Err(format!(
                "memory filename '{filename}' must be ASCII-printable"
            ));
        }
    }
    if filename.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(format!(
            "memory filename '{filename}' must not contain '..' segments"
        ));
    }
    if filename.ends_with('.') || filename.ends_with(' ') || filename.ends_with('\t') {
        // Trailing dots/whitespace are silently stripped on Windows,
        // so `MEMORY.md.` and `MEMORY.md` resolve to the same file.
        return Err(format!(
            "memory filename '{filename}' must not end with '.' or whitespace"
        ));
    }
    let lower = filename.to_ascii_lowercase();
    if RESERVED_INDEX_NAMES.contains(&lower.as_str()) {
        return Err(format!(
            "memory filename '{filename}' collides with a reserved index file"
        ));
    }
    // Stem before the first dot — Windows treats `con.anything` as the
    // CON device.
    let stem = lower.split('.').next().unwrap_or(&lower);
    if WINDOWS_RESERVED_STEMS.contains(&stem) {
        return Err(format!(
            "memory filename '{filename}' uses a Windows-reserved device name"
        ));
    }
    Ok(())
}

/// Defense-in-depth: after validation, confirm the joined path resolves
/// inside `memory_dir`. Tolerates either the parent or the file not
/// existing yet by canonicalizing the directory and checking that the
/// would-be file's parent matches.
///
/// Also rejects when the leaf itself is a symlink — `std::fs::write`
/// would otherwise dereference it and overwrite a file outside the
/// memory dir even when both the dir and the parent are clean.
fn ensure_path_within(memory_dir: &Path, file_path: &Path) -> Result<(), String> {
    let dir_canon = std::fs::canonicalize(memory_dir)
        .map_err(|e| format!("Failed to canonicalize memory dir: {e}"))?;
    let parent = file_path
        .parent()
        .ok_or_else(|| "memory file path has no parent".to_string())?;
    let parent_canon = std::fs::canonicalize(parent)
        .map_err(|e| format!("Failed to canonicalize memory file parent: {e}"))?;
    if !parent_canon.starts_with(&dir_canon) {
        return Err(format!(
            "memory file path escapes memory directory: {}",
            file_path.display()
        ));
    }

    // Refuse a symlink leaf: `std::fs::write` follows it and would
    // overwrite the link target, which can sit outside `memory_dir`.
    // `symlink_metadata` does not traverse, so this catches the
    // pre-planted-symlink case.
    if let Ok(meta) = std::fs::symlink_metadata(file_path)
        && meta.file_type().is_symlink()
    {
        return Err(format!(
            "memory file path is a symlink: {}",
            file_path.display()
        ));
    }
    Ok(())
}

/// Write a memory file and update the index atomically.
///
/// Returns the path of the written memory file.
pub fn write_memory(
    memory_dir: &Path,
    filename: &str,
    meta: &MemoryMeta,
    content: &str,
) -> Result<PathBuf, String> {
    validate_memory_filename(filename)?;
    let _ = std::fs::create_dir_all(memory_dir);

    // Step 1: Write the memory file with frontmatter.
    let type_str = match &meta.memory_type {
        Some(MemoryType::User) => "user",
        Some(MemoryType::Feedback) => "feedback",
        Some(MemoryType::Project) => "project",
        Some(MemoryType::Reference) => "reference",
        None => "user",
    };

    let file_content = format!(
        "---\nname: {}\ndescription: {}\ntype: {}\n---\n\n{}",
        meta.name, meta.description, type_str, content
    );

    let file_path = memory_dir.join(filename);
    ensure_path_within(memory_dir, &file_path)?;
    std::fs::write(&file_path, &file_content)
        .map_err(|e| format!("Failed to write memory file: {e}"))?;

    // Step 2: Update MEMORY.md index.
    update_index(memory_dir, filename, &meta.name, &meta.description)?;

    Ok(file_path)
}

/// Write a team-shared memory entry, with author + ISO-8601 timestamp.
///
/// This is the only sanctioned path for adding to
/// `<project>/.agent/team-memory/`. The model's own file-write tools
/// route through `write_memory`, which is fine for the per-user
/// memory directory but must not be used to mutate team memory; see
/// [`super::is_team_memory_path`] for the matching guard predicate.
///
/// `force=false` makes this an append-only operation: a collision
/// against an existing filename returns `Err` with a descriptive
/// message. The slash-command handler can then prompt the user to
/// pick a new name or pass `--force`.
pub fn write_team_memory(
    team_memory_dir: &Path,
    filename: &str,
    meta: &MemoryMeta,
    content: &str,
    force: bool,
) -> Result<PathBuf, String> {
    validate_memory_filename(filename)?;
    let _ = std::fs::create_dir_all(team_memory_dir);

    let file_path = team_memory_dir.join(filename);
    ensure_path_within(team_memory_dir, &file_path)?;
    if file_path.exists() && !force {
        return Err(format!(
            "team-memory entry '{filename}' already exists. \
             Pick a different name or pass --force to overwrite."
        ));
    }

    let type_str = match &meta.memory_type {
        Some(MemoryType::User) => "user",
        Some(MemoryType::Feedback) => "feedback",
        Some(MemoryType::Project) => "project",
        Some(MemoryType::Reference) => "reference",
        None => "project",
    };

    let mut header = format!(
        "---\nname: {}\ndescription: {}\ntype: {}",
        meta.name, meta.description, type_str
    );
    if let Some(a) = &meta.author {
        header.push_str(&format!("\nauthor: {a}"));
    }
    if let Some(c) = &meta.created_at {
        header.push_str(&format!("\ncreated_at: {c}"));
    }
    header.push_str("\n---\n\n");

    let file_content = format!("{header}{content}");
    std::fs::write(&file_path, &file_content)
        .map_err(|e| format!("Failed to write team-memory file: {e}"))?;

    update_index(team_memory_dir, filename, &meta.name, &meta.description)?;

    Ok(file_path)
}

/// List filenames currently registered in the team-memory directory
/// (excluding `MEMORY.md`).
pub fn list_team_memory(team_memory_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(team_memory_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == "md")
                && path.file_name().is_some_and(|n| n != "MEMORY.md")
            {
                path.file_name().and_then(|n| n.to_str()).map(String::from)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// Remove a team-memory entry. Trims `.md` automatically if absent.
pub fn delete_team_memory(team_memory_dir: &Path, name_or_filename: &str) -> Result<(), String> {
    let filename = if name_or_filename.ends_with(".md") {
        name_or_filename.to_string()
    } else {
        format!("{name_or_filename}.md")
    };
    // Validate before delegating: `delete_memory` would otherwise
    // happily resolve `../../README.md` against `team_memory_dir`.
    validate_memory_filename(&filename)?;
    delete_memory(team_memory_dir, &filename)
}

/// Update the MEMORY.md index with a pointer to a memory file.
/// If an entry for this filename already exists, replace it.
fn update_index(
    memory_dir: &Path,
    filename: &str,
    name: &str,
    description: &str,
) -> Result<(), String> {
    let index_path = memory_dir.join("MEMORY.md");

    let existing = std::fs::read_to_string(&index_path).unwrap_or_default();

    // Build the new index line (under 150 chars).
    let mut line = format!("- [{}]({}) — {}", name, filename, description);
    if line.len() > MAX_INDEX_LINE_CHARS {
        line.truncate(MAX_INDEX_LINE_CHARS - 3);
        line.push_str("...");
    }

    // Replace existing entry for this filename, or append.
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|l| !l.contains(&format!("({})", filename)))
        .map(|l| l.to_string())
        .collect();

    lines.push(line);

    // Enforce max lines.
    if lines.len() > MAX_INDEX_LINES {
        lines.truncate(MAX_INDEX_LINES);
    }

    let new_index = lines.join("\n") + "\n";
    std::fs::write(&index_path, new_index).map_err(|e| format!("Failed to update index: {e}"))?;

    Ok(())
}

/// Remove a memory file and its index entry.
pub fn delete_memory(memory_dir: &Path, filename: &str) -> Result<(), String> {
    validate_memory_filename(filename)?;
    let file_path = memory_dir.join(filename);
    if file_path.exists() {
        // Defense-in-depth: even with a validated filename, confirm
        // the resolved path lives under `memory_dir` before unlinking.
        ensure_path_within(memory_dir, &file_path)?;
        std::fs::remove_file(&file_path).map_err(|e| format!("Failed to delete: {e}"))?;
    }

    // Remove from index.
    let index_path = memory_dir.join("MEMORY.md");
    if let Ok(existing) = std::fs::read_to_string(&index_path) {
        let filtered: Vec<&str> = existing
            .lines()
            .filter(|l| !l.contains(&format!("({})", filename)))
            .collect();
        let _ = std::fs::write(&index_path, filtered.join("\n") + "\n");
    }

    Ok(())
}

/// Rebuild MEMORY.md from the actual files in the memory directory.
/// Scans all .md files (except MEMORY.md itself), reads their frontmatter,
/// and regenerates the index.
///
/// The on-disk output is byte-stable: headers are sorted
/// lexicographically by filename before emitting, so two reindex runs
/// over the same set of files produce identical bytes regardless of
/// mtime drift across checkouts or filesystems. Without this the
/// scanner's mtime-first ordering would shuffle the index between
/// users on the same team.
pub fn rebuild_index(memory_dir: &Path) -> Result<(), String> {
    let mut headers = super::scanner::scan_memory_files(memory_dir);
    headers.sort_by(|a, b| a.filename.cmp(&b.filename));
    let index_path = memory_dir.join("MEMORY.md");

    let mut lines = Vec::new();
    for h in &headers {
        let name = h
            .meta
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or(&h.filename);
        let desc = h
            .meta
            .as_ref()
            .map(|m| m.description.as_str())
            .unwrap_or("");

        let mut line = format!("- [{}]({}) — {}", name, h.filename, desc);
        if line.len() > MAX_INDEX_LINE_CHARS {
            line.truncate(MAX_INDEX_LINE_CHARS - 3);
            line.push_str("...");
        }
        lines.push(line);
    }

    if lines.len() > MAX_INDEX_LINES {
        lines.truncate(MAX_INDEX_LINES);
    }

    let content = lines.join("\n") + "\n";
    std::fs::write(&index_path, content).map_err(|e| format!("Failed to write index: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta() -> MemoryMeta {
        MemoryMeta {
            name: "Test Memory".to_string(),
            description: "A test memory file".to_string(),
            memory_type: Some(MemoryType::User),
            author: None,
            created_at: None,
        }
    }

    #[test]
    fn test_write_memory_creates_file_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta();
        let path = write_memory(dir.path(), "test.md", &meta, "Hello world").unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("name: Test Memory"));
        assert!(content.contains("type: user"));
        assert!(content.contains("Hello world"));

        // Index should exist and contain a pointer.
        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        assert!(index.contains("[Test Memory](test.md)"));
    }

    #[test]
    fn test_write_memory_updates_existing_index_entry() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta();
        write_memory(dir.path(), "test.md", &meta, "version 1").unwrap();

        let meta2 = MemoryMeta {
            name: "Updated".to_string(),
            description: "Updated description".to_string(),
            memory_type: Some(MemoryType::Feedback),
            author: None,
            created_at: None,
        };
        write_memory(dir.path(), "test.md", &meta2, "version 2").unwrap();

        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        // Should have only one entry for test.md (replaced, not duplicated).
        assert_eq!(index.matches("test.md").count(), 1);
        assert!(index.contains("[Updated](test.md)"));
    }

    #[test]
    fn test_delete_memory() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta();
        write_memory(dir.path(), "test.md", &meta, "content").unwrap();

        delete_memory(dir.path(), "test.md").unwrap();

        assert!(!dir.path().join("test.md").exists());
        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        assert!(!index.contains("test.md"));
    }

    #[test]
    fn test_delete_nonexistent_memory() {
        let dir = tempfile::tempdir().unwrap();
        // Should not error even if file doesn't exist.
        assert!(delete_memory(dir.path(), "nope.md").is_ok());
    }

    #[test]
    fn test_rebuild_index() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta();
        write_memory(dir.path(), "one.md", &meta, "first").unwrap();

        let meta2 = MemoryMeta {
            name: "Second".to_string(),
            description: "Second file".to_string(),
            memory_type: Some(MemoryType::Project),
            author: None,
            created_at: None,
        };
        write_memory(dir.path(), "two.md", &meta2, "second").unwrap();

        // Corrupt the index.
        std::fs::write(dir.path().join("MEMORY.md"), "garbage").unwrap();

        // Rebuild should restore it.
        rebuild_index(dir.path()).unwrap();
        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        assert!(index.contains("one.md"));
        assert!(index.contains("two.md"));
    }

    #[test]
    fn rebuild_index_is_byte_stable_across_runs() {
        // Same directory, write two entries, then bump the mtime of
        // one of them. mtime-driven ordering would shuffle output;
        // lexicographic ordering keeps it byte-stable.
        let dir = tempfile::tempdir().unwrap();
        let meta_a = MemoryMeta {
            name: "Alpha".into(),
            description: "alpha".into(),
            memory_type: Some(MemoryType::User),
            author: None,
            created_at: None,
        };
        let meta_b = MemoryMeta {
            name: "Beta".into(),
            description: "beta".into(),
            memory_type: Some(MemoryType::User),
            author: None,
            created_at: None,
        };
        write_memory(dir.path(), "alpha.md", &meta_a, "alpha-body").unwrap();
        write_memory(dir.path(), "beta.md", &meta_b, "beta-body").unwrap();

        rebuild_index(dir.path()).unwrap();
        let first = std::fs::read(dir.path().join("MEMORY.md")).unwrap();

        // Touch beta.md so its mtime sorts ahead of alpha.md. An
        // mtime-ordered reindex would emit beta before alpha; the
        // lexicographic ordering must override that.
        let beta_path = dir.path().join("beta.md");
        let body = std::fs::read_to_string(&beta_path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&beta_path, body).unwrap();

        rebuild_index(dir.path()).unwrap();
        let second = std::fs::read(dir.path().join("MEMORY.md")).unwrap();

        assert_eq!(
            first, second,
            "rebuild_index must be byte-stable across mtime drift"
        );

        // And the order is lexicographic — alpha before beta.
        let s = String::from_utf8(second).unwrap();
        let alpha_pos = s.find("alpha.md").unwrap();
        let beta_pos = s.find("beta.md").unwrap();
        assert!(alpha_pos < beta_pos, "alpha must come before beta");
    }

    fn team_meta() -> MemoryMeta {
        MemoryMeta {
            name: "Deploy".into(),
            description: "team deploy steps".into(),
            memory_type: Some(MemoryType::Project),
            author: Some("alice@example.com".into()),
            created_at: Some("2025-01-02T03:04:05Z".into()),
        }
    }

    #[test]
    fn test_write_team_memory_writes_frontmatter_with_author() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_team_memory(dir.path(), "deploy.md", &team_meta(), "ship it", false)
            .expect("write succeeds");
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("author: alice@example.com"));
        assert!(body.contains("created_at: 2025-01-02T03:04:05Z"));
        assert!(body.contains("ship it"));
        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        assert!(index.contains("[Deploy](deploy.md)"));
    }

    #[test]
    fn test_write_team_memory_refuses_collision_without_force() {
        let dir = tempfile::tempdir().unwrap();
        write_team_memory(dir.path(), "deploy.md", &team_meta(), "v1", false).unwrap();
        let err =
            write_team_memory(dir.path(), "deploy.md", &team_meta(), "v2", false).unwrap_err();
        assert!(err.contains("already exists"));
        // Body unchanged.
        let body = std::fs::read_to_string(dir.path().join("deploy.md")).unwrap();
        assert!(body.contains("v1"));
        assert!(!body.contains("v2"));
    }

    #[test]
    fn test_write_team_memory_overwrites_with_force() {
        let dir = tempfile::tempdir().unwrap();
        write_team_memory(dir.path(), "deploy.md", &team_meta(), "v1", false).unwrap();
        write_team_memory(dir.path(), "deploy.md", &team_meta(), "v2", true).unwrap();
        let body = std::fs::read_to_string(dir.path().join("deploy.md")).unwrap();
        assert!(body.contains("v2"));
    }

    #[test]
    fn test_list_team_memory_skips_index() {
        let dir = tempfile::tempdir().unwrap();
        write_team_memory(dir.path(), "a.md", &team_meta(), "a", false).unwrap();
        write_team_memory(dir.path(), "b.md", &team_meta(), "b", false).unwrap();
        let names = list_team_memory(dir.path());
        assert_eq!(names, vec!["a.md", "b.md"]);
    }

    #[test]
    fn test_delete_team_memory_accepts_bare_name() {
        let dir = tempfile::tempdir().unwrap();
        write_team_memory(dir.path(), "deploy.md", &team_meta(), "x", false).unwrap();
        delete_team_memory(dir.path(), "deploy").unwrap();
        assert!(!dir.path().join("deploy.md").exists());
    }

    // ---- filename validation / path traversal ----

    #[test]
    fn validate_rejects_empty() {
        assert!(validate_memory_filename("").is_err());
    }

    #[test]
    fn validate_rejects_dot_and_dotdot() {
        assert!(validate_memory_filename(".").is_err());
        assert!(validate_memory_filename("..").is_err());
    }

    #[test]
    fn validate_rejects_path_separators() {
        assert!(validate_memory_filename("foo/bar.md").is_err());
        assert!(validate_memory_filename("foo\\bar.md").is_err());
        assert!(validate_memory_filename("/abs.md").is_err());
        assert!(validate_memory_filename("\\abs.md").is_err());
    }

    #[test]
    fn validate_rejects_traversal_segments() {
        // Even without a separator, '..' alone is rejected via the
        // dot-handling branch.
        assert!(validate_memory_filename("..").is_err());
        // With separators, every '..' segment is rejected by the
        // separator check first.
        assert!(validate_memory_filename("../README.md").is_err());
        assert!(validate_memory_filename("../../etc/passwd").is_err());
        assert!(validate_memory_filename("a/../b.md").is_err());
    }

    #[test]
    fn validate_rejects_nul_and_newline() {
        assert!(validate_memory_filename("foo\0.md").is_err());
        assert!(validate_memory_filename("foo\n.md").is_err());
        assert!(validate_memory_filename("foo\r.md").is_err());
        assert!(validate_memory_filename("foo\t.md").is_err());
    }

    #[test]
    fn validate_rejects_non_ascii() {
        assert!(validate_memory_filename("café.md").is_err());
    }

    #[test]
    fn validate_rejects_overlong() {
        let huge = "a".repeat(MAX_FILENAME_LEN + 1) + ".md";
        assert!(validate_memory_filename(&huge).is_err());
    }

    #[test]
    fn validate_accepts_normal_names() {
        assert!(validate_memory_filename("deploy.md").is_ok());
        assert!(validate_memory_filename("team-deploy_2025.md").is_ok());
        assert!(validate_memory_filename("a.md").is_ok());
    }

    #[test]
    fn validate_rejects_index_collisions_case_insensitive() {
        assert!(validate_memory_filename("MEMORY.md").is_err());
        assert!(validate_memory_filename("memory.md").is_err());
        assert!(validate_memory_filename("Memory.md").is_err());
        assert!(validate_memory_filename("README.md").is_err());
        assert!(validate_memory_filename("readme.md").is_err());
    }

    #[test]
    fn validate_rejects_trailing_dot_or_whitespace() {
        assert!(validate_memory_filename("deploy.md.").is_err());
        assert!(validate_memory_filename("deploy.md ").is_err());
        assert!(validate_memory_filename("deploy.md\t").is_err());
    }

    #[test]
    fn validate_rejects_windows_reserved_names() {
        // Bare device names.
        assert!(validate_memory_filename("CON").is_err());
        assert!(validate_memory_filename("con").is_err());
        assert!(validate_memory_filename("PRN").is_err());
        assert!(validate_memory_filename("AUX").is_err());
        assert!(validate_memory_filename("NUL").is_err());
        // With extension — Windows still treats these as the device.
        assert!(validate_memory_filename("con.md").is_err());
        assert!(validate_memory_filename("aux.md").is_err());
        assert!(validate_memory_filename("COM1.md").is_err());
        assert!(validate_memory_filename("com9.md").is_err());
        assert!(validate_memory_filename("lpt1.md").is_err());
        assert!(validate_memory_filename("LPT9.md").is_err());
        // Sanity: similar names that aren't reserved still pass.
        assert!(validate_memory_filename("console.md").is_ok());
        assert!(validate_memory_filename("comma.md").is_ok());
    }

    #[test]
    fn delete_team_memory_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        // Plant a sibling file we want to confirm survives the attempt.
        let outside = dir.path().parent().unwrap().join("VICTIM.md");
        std::fs::write(&outside, "do not delete").unwrap();

        // Create the team-memory dir.
        let team_dir = dir.path().join("team");
        std::fs::create_dir_all(&team_dir).unwrap();

        // Bare name `../VICTIM` becomes `../VICTIM.md` after the
        // suffix trim — must be rejected.
        let err = delete_team_memory(&team_dir, "../VICTIM").unwrap_err();
        assert!(
            err.contains("path separators") || err.contains(".."),
            "unexpected error: {err}"
        );

        // Filename form must also be rejected.
        let err = delete_team_memory(&team_dir, "../VICTIM.md").unwrap_err();
        assert!(
            err.contains("path separators") || err.contains(".."),
            "unexpected error: {err}"
        );

        // Embedded NUL is rejected.
        assert!(delete_team_memory(&team_dir, "deploy\0").is_err());
        // Embedded newline is rejected.
        assert!(delete_team_memory(&team_dir, "deploy\nfoo").is_err());
        // Nested subdir rejected.
        assert!(delete_team_memory(&team_dir, "sub/dir").is_err());
        // Leading slash rejected.
        assert!(delete_team_memory(&team_dir, "/etc/passwd").is_err());

        // The outside file still exists.
        assert!(outside.exists(), "traversal deleted a file outside dir");
        // Cleanup.
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn write_team_memory_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        // `../foo.md` would land outside the dir; reject before write.
        let err =
            write_team_memory(dir.path(), "../escape.md", &team_meta(), "x", false).unwrap_err();
        assert!(
            err.contains("path separators") || err.contains(".."),
            "unexpected error: {err}"
        );
        assert!(!dir.path().parent().unwrap().join("escape.md").exists());
    }

    #[test]
    fn write_memory_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_memory(dir.path(), "../escape.md", &test_meta(), "x").unwrap_err();
        assert!(
            err.contains("path separators") || err.contains(".."),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn delete_memory_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        // Plant a victim file in the parent directory.
        let outside = dir.path().parent().unwrap().join("VICTIM_DEL.md");
        std::fs::write(&outside, "do not delete").unwrap();

        let err = delete_memory(dir.path(), "../VICTIM_DEL.md").unwrap_err();
        assert!(
            err.contains("path separators") || err.contains(".."),
            "unexpected error: {err}"
        );
        assert!(outside.exists());
        let _ = std::fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn write_memory_refuses_symlink_leaf() {
        // Pre-plant a symlink at the target file path that points to
        // a file outside the memory dir. Writing must be refused so
        // the link target stays untouched.
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().parent().unwrap().join("SYMLINK_VICTIM.md");
        std::fs::write(&outside, "untouched").unwrap();
        let link = dir.path().join("victim.md");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let err = write_memory(dir.path(), "victim.md", &test_meta(), "evil").unwrap_err();
        assert!(
            err.to_lowercase().contains("symlink"),
            "unexpected error: {err}"
        );
        // Outside file content unchanged.
        let body = std::fs::read_to_string(&outside).unwrap();
        assert_eq!(body, "untouched");
        // Symlink itself unchanged (still points at OUTSIDE.md).
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink());
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn write_team_memory_refuses_symlink_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().parent().unwrap().join("TEAM_SYMLINK_VICTIM.md");
        std::fs::write(&outside, "untouched").unwrap();
        let link = dir.path().join("link.md");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        // `force=true` makes the writer willing to overwrite — but the
        // symlink-leaf guard must still refuse.
        let err = write_team_memory(dir.path(), "link.md", &team_meta(), "evil", true).unwrap_err();
        assert!(
            err.to_lowercase().contains("symlink"),
            "unexpected error: {err}"
        );
        let body = std::fs::read_to_string(&outside).unwrap();
        assert_eq!(body, "untouched");
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn test_index_line_length_cap() {
        let dir = tempfile::tempdir().unwrap();
        let meta = MemoryMeta {
            name: "A".repeat(200),
            description: "B".repeat(200),
            memory_type: Some(MemoryType::User),
            author: None,
            created_at: None,
        };
        write_memory(dir.path(), "long.md", &meta, "content").unwrap();

        let index = std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap();
        for line in index.lines() {
            assert!(line.len() <= MAX_INDEX_LINE_CHARS + 3); // +3 for "..."
        }
    }
}
