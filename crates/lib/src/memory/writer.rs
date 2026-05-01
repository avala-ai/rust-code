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

/// Write a memory file and update the index atomically.
///
/// Returns the path of the written memory file.
pub fn write_memory(
    memory_dir: &Path,
    filename: &str,
    meta: &MemoryMeta,
    content: &str,
) -> Result<PathBuf, String> {
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
    let _ = std::fs::create_dir_all(team_memory_dir);

    let file_path = team_memory_dir.join(filename);
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
    let file_path = memory_dir.join(filename);
    if file_path.exists() {
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
pub fn rebuild_index(memory_dir: &Path) -> Result<(), String> {
    let headers = super::scanner::scan_memory_files(memory_dir);
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
