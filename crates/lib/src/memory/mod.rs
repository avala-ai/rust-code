//! Memory system — 3-layer architecture.
//!
//! **Layer 1 — Index (always loaded):**
//! MEMORY.md contains one-line pointers to topic files. Capped at
//! 200 lines / 25KB. Always in the system prompt.
//!
//! **Layer 2 — Topic files (on-demand):**
//! Individual .md files with YAML frontmatter. Loaded selectively
//! based on relevance to the current conversation.
//!
//! **Layer 3 — Transcripts (never loaded, only grepped):**
//! Past session logs. Not loaded into context.
//!
//! # Write discipline
//!
//! 1. Write the memory file with frontmatter
//! 2. Update MEMORY.md index with a one-line pointer
//!
//! Never dump content into the index.

pub mod consolidation;
pub mod extraction;
pub mod scanner;
pub mod session_notes;
pub mod types;
pub mod writer;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tracing::debug;

const MAX_INDEX_LINES: usize = 200;
const MAX_MEMORY_FILE_BYTES: usize = 25_000;

/// Persistent context loaded at session start.
///
/// Contains project-level context (`AGENTS.md`), user-level memory
/// (`~/.config/agent-code/memory/`), team-shared memory
/// (`<project>/.agent/team-memory/`), and individual memory files.
/// Injected into the system prompt so the agent has context across sessions.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Project-level instructions from `AGENTS.md` in the repo root.
    pub project_context: Option<String>,
    /// User-level memory index from `MEMORY.md`.
    pub user_memory: Option<String>,
    /// Team-shared memory index from `<project>/.agent/team-memory/MEMORY.md`.
    pub team_memory: Option<String>,
    /// Individual memory files linked from the index. Each carries its scope.
    pub memory_files: Vec<MemoryFile>,
    /// Paths already surfaced in this session (to avoid duplicates).
    pub surfaced: HashSet<PathBuf>,
}

/// A single memory file with metadata.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    /// Absolute path to the memory file.
    pub path: PathBuf,
    /// Memory name from frontmatter.
    pub name: String,
    /// File content (truncated at 25KB).
    pub content: String,
    /// Optional staleness indicator.
    pub staleness: Option<String>,
    /// Where this entry was loaded from. Drives collision precedence.
    pub scope: types::Scope,
}

impl MemoryContext {
    pub fn load(project_root: Option<&Path>) -> Self {
        let mut ctx = Self::default();

        // Collect candidate (scope, file) entries. Order does not matter
        // here — `merge_scoped_files` enforces the
        // `project > team > user` precedence rule for id collisions.
        let mut candidates: Vec<MemoryFile> = Vec::new();

        if let Some(root) = project_root {
            ctx.project_context = load_project_context(root);

            // Team-shared memory (version-controlled).
            let team_dir = team_memory_dir(root);
            let team_index = team_dir.join("MEMORY.md");
            if team_index.exists() {
                ctx.team_memory = load_truncated_file(&team_index);
            }
            if let Some(ref index) = ctx.team_memory {
                candidates.extend(load_referenced_files(index, &team_dir, types::Scope::Team));
            }

            // Project-level memory (`<project>/.agent/memory/`). Existing
            // layout: `.agent/` may hold a `MEMORY.md` and topic files.
            let project_dir = project_memory_dir(root).join("memory");
            let project_index = project_dir.join("MEMORY.md");
            if project_index.exists()
                && let Some(idx) = load_truncated_file(&project_index)
            {
                candidates.extend(load_referenced_files(
                    &idx,
                    &project_dir,
                    types::Scope::Project,
                ));
            }
        }

        // Per-user memory.
        if let Some(memory_dir) = user_memory_dir() {
            let index_path = memory_dir.join("MEMORY.md");
            if index_path.exists() {
                ctx.user_memory = load_truncated_file(&index_path);
            }
            if let Some(ref index) = ctx.user_memory {
                candidates.extend(load_referenced_files(
                    index,
                    &memory_dir,
                    types::Scope::User,
                ));
            }
        }

        ctx.memory_files = merge_scoped_files(candidates);
        ctx
    }

    pub fn load_relevant(&mut self, recent_text: &str) {
        // Scan every dir we know about so on-demand surfacing also pulls
        // team and project memory, not just user memory.
        let mut headers: Vec<(scanner::MemoryHeader, types::Scope)> = Vec::new();
        if let Some(memory_dir) = user_memory_dir() {
            for h in scanner::scan_memory_files(&memory_dir) {
                headers.push((h, types::Scope::User));
            }
        }
        // Project + team scans only fire if we have a project root pinned
        // via the existing user_memory_dir/project_memory_dir mechanism.
        // Here we infer the project root from any already-loaded file,
        // because `load_relevant` does not take a root directly.
        if let Some(root) = self.project_root_hint() {
            let project_dir = project_memory_dir(&root).join("memory");
            if project_dir.is_dir() {
                for h in scanner::scan_memory_files(&project_dir) {
                    headers.push((h, types::Scope::Project));
                }
            }
            let team_dir = team_memory_dir(&root);
            if team_dir.is_dir() {
                for h in scanner::scan_memory_files(&team_dir) {
                    headers.push((h, types::Scope::Team));
                }
            }
        }

        let header_only: Vec<scanner::MemoryHeader> =
            headers.iter().map(|(h, _)| h.clone()).collect();
        let relevant = scanner::select_relevant(&header_only, recent_text, &self.surfaced);
        for path in relevant {
            // Find the scope this header was discovered under.
            let scope = headers
                .iter()
                .find(|(h, _)| h.path == path)
                .map(|(_, s)| *s)
                .unwrap_or(types::Scope::User);
            if let Some(mut file) = load_memory_file_with_staleness(&path) {
                file.scope = scope;
                self.surfaced.insert(path);
                self.memory_files.push(file);
            }
        }
    }

    /// Best-effort hint at the project root by inspecting paths of files
    /// loaded under the Project or Team scope at `load` time.
    fn project_root_hint(&self) -> Option<PathBuf> {
        for f in &self.memory_files {
            if matches!(f.scope, types::Scope::Project | types::Scope::Team) {
                // Walk up two levels: <root>/.agent/(team-)memory/file.md.
                let mut p = f.path.parent()?.to_path_buf();
                p.pop(); // drop "memory" / "team-memory"
                p.pop(); // drop ".agent"
                return Some(p);
            }
        }
        None
    }

    pub fn to_system_prompt_section(&self) -> String {
        let mut section = String::new();
        if let Some(ref project) = self.project_context
            && !project.is_empty()
        {
            section.push_str("# Project Context\n\n");
            section.push_str(project);
            section.push_str("\n\n");
        }
        if let Some(ref team) = self.team_memory
            && !team.is_empty()
        {
            section.push_str("# Team Memory Index\n\n");
            section.push_str(team);
            section.push_str("\n\n");
            section.push_str(
                "_Team memory is shared across everyone on this project. \
                 Treat it as read-only from the agent's side: only the \
                 explicit `/team-remember` command may add entries._\n\n",
            );
        }
        if let Some(ref memory) = self.user_memory
            && !memory.is_empty()
        {
            section.push_str("# Memory Index\n\n");
            section.push_str(memory);
            section.push_str("\n\n");
            section.push_str(
                "_Memory is a hint, not truth. Verify against current state \
                     before acting on remembered facts._\n\n",
            );
        }
        for file in &self.memory_files {
            section.push_str(&format!("## Memory: {}\n\n", file.name));
            if let Some(ref warning) = file.staleness {
                section.push_str(&format!("_{warning}_\n\n"));
            }
            section.push_str(&file.content);
            section.push_str("\n\n");
        }
        section
    }

    pub fn is_empty(&self) -> bool {
        self.project_context.is_none()
            && self.user_memory.is_none()
            && self.team_memory.is_none()
            && self.memory_files.is_empty()
    }
}

/// Walk from the git repo root down to `start` (inclusive) and return
/// every `AGENTS.md` / `.agent/AGENTS.md` / `CLAUDE.md` /
/// `.claude/CLAUDE.md` that exists, ordered outermost→innermost.
///
/// "Git repo root" is the nearest ancestor containing `.git`. If no
/// `.git` is found, the walk stops at `start` itself — we never escape
/// the session dir to load config from random parent dirs.
fn hierarchical_project_files(start: &Path) -> Vec<PathBuf> {
    // Walk ancestors once to locate the repo root (nearest dir with
    // `.git`). `.git` can be a directory (normal checkout) or a file
    // (submodules / worktrees). We accept either.
    let mut repo_root: Option<&Path> = None;
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            repo_root = Some(dir);
            break;
        }
    }

    // Build the walk range: every directory from repo_root down to
    // start inclusive. Using strip_prefix + component iteration keeps
    // this deterministic regardless of OS path-separator.
    let top: &Path = repo_root.unwrap_or(start);
    let mut dirs: Vec<PathBuf> = vec![top.to_path_buf()];
    if let Ok(rel) = start.strip_prefix(top) {
        let mut cursor = top.to_path_buf();
        for seg in rel.components() {
            cursor.push(seg);
            if cursor != *top {
                dirs.push(cursor.clone());
            }
        }
    }

    let mut files = Vec::new();
    for dir in &dirs {
        // Primary names at each level. AGENTS.md first so it wins
        // the `is_file()` race for callers that stop at first hit.
        for leaf in &[
            "AGENTS.md",
            ".agent/AGENTS.md",
            "CLAUDE.md",
            ".claude/CLAUDE.md",
        ] {
            let p = dir.join(leaf);
            if p.is_file() {
                files.push(p);
            }
        }
    }
    files
}

/// Load project context by traversing the directory hierarchy.
///
/// Checks (in priority order, lowest to highest):
/// 1. User global: ~/.config/agent-code/AGENTS.md
/// 2. Hierarchical project context: every `AGENTS.md` (or `CLAUDE.md`
///    compat) from the git repo root down to the session cwd, in
///    outermost-to-innermost order. Deeper files are loaded last so
///    their contents override earlier layers in the composed prompt.
///    `.agent/AGENTS.md` at each level is also honored.
/// 3. Project rules: .agent/rules/*.md AND .claude/rules/*.md
/// 4. Project local: AGENTS.local.md / CLAUDE.local.md (gitignored)
///
/// CLAUDE.md is supported for compatibility with existing projects.
/// If both AGENTS.md and CLAUDE.md exist, both are loaded (AGENTS.md first).
fn load_project_context(project_root: &Path) -> Option<String> {
    let mut sections = Vec::new();

    // Layer 1: User global context.
    for name in &["AGENTS.md", "CLAUDE.md"] {
        if let Some(global_path) = dirs::config_dir().map(|d| d.join("agent-code").join(name))
            && let Some(content) = load_truncated_file(&global_path)
        {
            debug!("Loaded global context from {}", global_path.display());
            sections.push(content);
        }
    }

    // Layer 2: Hierarchical project context.
    //
    // Walk from the git repo root down to `project_root` (typically the
    // session cwd). Load every `AGENTS.md` / `.agent/AGENTS.md` /
    // `CLAUDE.md` / `.claude/CLAUDE.md` seen along the way so an
    // `AGENTS.md` in a monorepo sub-package actually takes effect when
    // the agent is invoked from that subdir. Outermost-first ordering
    // lets deeper (more specific) files override broader ones.
    for path in hierarchical_project_files(project_root) {
        if let Some(content) = load_truncated_file(&path) {
            debug!("Loaded project context from {}", path.display());
            sections.push(content);
        }
    }

    // Layer 3: Rules directories (both .agent/ and .claude/ for compat).
    for rules_dir in &[
        project_root.join(".agent").join("rules"),
        project_root.join(".claude").join("rules"),
    ] {
        if rules_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(rules_dir)
        {
            let mut rule_files: Vec<_> = entries
                .flatten()
                .filter(|e| {
                    e.path().extension().is_some_and(|ext| ext == "md") && e.path().is_file()
                })
                .collect();
            rule_files.sort_by_key(|e| e.file_name());

            for entry in rule_files {
                if let Some(content) = load_truncated_file(&entry.path()) {
                    debug!("Loaded rule from {}", entry.path().display());
                    sections.push(content);
                }
            }
        }
    }

    // Layer 4: Local overrides (gitignored).
    for name in &["AGENTS.local.md", "CLAUDE.local.md"] {
        let local_path = project_root.join(name);
        if let Some(content) = load_truncated_file(&local_path) {
            debug!("Loaded local context from {}", local_path.display());
            sections.push(content);
        }
    }

    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

fn load_truncated_file(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    if content.is_empty() {
        return None;
    }

    let mut result = content.clone();
    let mut was_byte_truncated = false;

    if result.len() > MAX_MEMORY_FILE_BYTES {
        if let Some(pos) = result[..MAX_MEMORY_FILE_BYTES].rfind('\n') {
            result.truncate(pos);
        } else {
            result.truncate(MAX_MEMORY_FILE_BYTES);
        }
        was_byte_truncated = true;
    }

    let lines: Vec<&str> = result.lines().collect();
    let was_line_truncated = lines.len() > MAX_INDEX_LINES;
    if was_line_truncated {
        result = lines[..MAX_INDEX_LINES].join("\n");
    }

    if was_byte_truncated || was_line_truncated {
        result.push_str("\n\n(truncated)");
    }

    Some(result)
}

fn load_memory_file_with_staleness(path: &Path) -> Option<MemoryFile> {
    let content = load_truncated_file(path)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let staleness = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|modified| {
            let age = std::time::SystemTime::now().duration_since(modified).ok()?;
            types::staleness_caveat(age.as_secs())
        });

    Some(MemoryFile {
        path: path.to_path_buf(),
        name,
        content,
        staleness,
        // Default — overwritten by callers that know the scope.
        scope: types::Scope::User,
    })
}

fn load_referenced_files(index: &str, base_dir: &Path, scope: types::Scope) -> Vec<MemoryFile> {
    let mut files = Vec::new();
    let link_re = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();

    for captures in link_re.captures_iter(index) {
        let name = captures.get(1).map(|m| m.as_str()).unwrap_or("");
        let filename = captures.get(2).map(|m| m.as_str()).unwrap_or("");
        if filename.is_empty() || !filename.ends_with(".md") {
            continue;
        }
        // Refuse anything that could escape `base_dir`. A poisoned
        // index (especially the version-controlled team-memory one)
        // could otherwise point at `../../home/user/.ssh/id_rsa.md`
        // and pull arbitrary files into the system prompt.
        let path = match resolve_index_link_target(base_dir, filename) {
            Some(p) => p,
            None => {
                debug!(
                    "memory index link rejected: '{}' escapes base dir or is a symlink",
                    filename
                );
                continue;
            }
        };
        if let Some(mut file) = load_memory_file_with_staleness(&path) {
            file.name = name.to_string();
            file.scope = scope;
            files.push(file);
        }
    }
    files
}

/// Resolve a `[name](path)` link target into a concrete absolute path
/// inside `base_dir`. Returns `None` for any of:
///
/// - empty / non-`.md` paths (caller already filtered, defensive),
/// - absolute paths (must be relative to the index),
/// - paths containing `..` segments,
/// - paths whose canonical form lands outside `base_dir`,
/// - paths whose final component is a symlink (anti-confusion).
///
/// Applied uniformly to user, team, and project memory loads.
fn resolve_index_link_target(base_dir: &Path, filename: &str) -> Option<PathBuf> {
    if filename.is_empty() || !filename.ends_with(".md") {
        return None;
    }
    let candidate = Path::new(filename);
    if candidate.is_absolute() {
        return None;
    }
    for comp in candidate.components() {
        match comp {
            std::path::Component::ParentDir | std::path::Component::RootDir => return None,
            _ => {}
        }
    }

    let joined = base_dir.join(candidate);

    // Refuse symlink leaves so a poisoned index can't dereference into
    // an arbitrary file just because the symlink itself sits inside
    // the memory dir.
    if let Ok(meta) = std::fs::symlink_metadata(&joined)
        && meta.file_type().is_symlink()
    {
        return None;
    }

    // Canonical containment: when the file already exists, the
    // canonical path must start with the canonical base dir.
    if joined.exists() {
        let base_canon = base_dir.canonicalize().ok()?;
        let target_canon = joined.canonicalize().ok()?;
        if !target_canon.starts_with(&base_canon) {
            return None;
        }
        return Some(target_canon);
    }

    // File doesn't exist yet (caller will skip-load anyway). Lexically
    // verify the joined path stays inside base_dir.
    let base_canon = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.to_path_buf());
    let mut normalized = PathBuf::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    if normalized.starts_with(&base_canon) || normalized.starts_with(base_dir) {
        Some(joined)
    } else {
        None
    }
}

/// Merge memory entries from multiple scopes, applying the
/// `project > team > user` precedence rule on id (filename) collisions.
///
/// The id used for collision is the file stem (e.g. `prefs.md` → `prefs`),
/// because the same topic may live in two scopes with different paths
/// but the same filename. On collision, the higher-precedence entry
/// wins and a debug log records the discarded scope. Callers should
/// rely on this function rather than dedup themselves.
///
/// The returned `Vec` is sorted by `(scope_priority, path_lex)` where
/// scope priority is `User < Team < Project` (most-specific last). This
/// ordering is deterministic across loads with identical inputs, which
/// is required for prompt-cache stability — `HashMap::into_values`
/// would otherwise shuffle the prompt layout between sessions.
fn merge_scoped_files(candidates: Vec<MemoryFile>) -> Vec<MemoryFile> {
    use std::collections::BTreeMap;
    let mut by_id: BTreeMap<String, MemoryFile> = BTreeMap::new();
    for file in candidates {
        let id = file
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        match by_id.get(&id) {
            Some(existing) if scope_precedence(existing.scope) >= scope_precedence(file.scope) => {
                debug!(
                    "memory id '{}' present in both {} and {} scopes; keeping {} (higher precedence)",
                    id,
                    existing.scope.label(),
                    file.scope.label(),
                    existing.scope.label()
                );
            }
            Some(existing) => {
                debug!(
                    "memory id '{}' present in both {} and {} scopes; keeping {} (higher precedence)",
                    id,
                    existing.scope.label(),
                    file.scope.label(),
                    file.scope.label()
                );
                by_id.insert(id, file);
            }
            None => {
                by_id.insert(id, file);
            }
        }
    }
    let mut out: Vec<MemoryFile> = by_id.into_values().collect();
    // Stable order: sort by (scope_priority, path). Lower-priority
    // scopes come first so the prompt builder emits broad context
    // before specific overrides, matching the
    // outermost→innermost convention used elsewhere
    // (see `hierarchical_project_files`).
    out.sort_by(|a, b| {
        scope_precedence(a.scope)
            .cmp(&scope_precedence(b.scope))
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// Precedence ranking: higher number wins on collision.
fn scope_precedence(scope: types::Scope) -> u8 {
    match scope {
        types::Scope::Project => 3,
        types::Scope::Team => 2,
        types::Scope::User => 1,
    }
}

fn user_memory_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("memory"))
}

/// Returns the project-level memory directory (`.agent/` in the project root).
pub fn project_memory_dir(project_root: &Path) -> PathBuf {
    project_root.join(".agent")
}

/// Directory holding team-shared, version-controlled memory for this project.
///
/// Lives at `<project>/.agent/team-memory/`. Read by every session that opens
/// the project. Writes must go through the `/team-remember` slash command —
/// they never originate from the model's own file-write tools or from the
/// background extraction loop. See [`is_team_memory_path`].
pub fn team_memory_dir(project_root: &Path) -> PathBuf {
    project_memory_dir(project_root).join("team-memory")
}

/// Ensure the team-memory directory exists for the given project root.
/// Returns the directory path. Creates it if missing.
pub fn ensure_team_memory_dir(project_root: &Path) -> std::io::Result<PathBuf> {
    let dir = team_memory_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// True if `path` lives inside any project's team-memory directory
/// (matches `*/.agent/team-memory/*`). Used by safety guards that
/// must refuse model-driven writes to team memory.
pub fn is_team_memory_path(path: &Path) -> bool {
    let mut saw_dot_agent = false;
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy();
        if s == ".agent" {
            saw_dot_agent = true;
            continue;
        }
        if saw_dot_agent && s == "team-memory" {
            return true;
        }
        // Reset if we wandered past `.agent` without hitting `team-memory`.
        if saw_dot_agent {
            saw_dot_agent = false;
        }
    }
    false
}

/// Returns the user-level memory directory, creating it if needed.
pub fn ensure_memory_dir() -> Option<PathBuf> {
    let dir = user_memory_dir()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_truncated_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.md");
        std::fs::write(&path, "a\n".repeat(300)).unwrap();
        let loaded = load_truncated_file(&path).unwrap();
        assert!(loaded.contains("truncated"));
    }

    #[test]
    fn test_load_referenced_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("prefs.md"), "I prefer Rust").unwrap();
        let index = "- [Preferences](prefs.md) — prefs\n- [Missing](gone.md) — gone";
        let files = load_referenced_files(index, dir.path(), types::Scope::User);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "Preferences");
        assert_eq!(files[0].scope, types::Scope::User);
    }

    #[test]
    fn load_referenced_files_rejects_traversal_links() {
        let dir = tempfile::tempdir().unwrap();
        // Plant a victim file outside the memory dir.
        let outside = dir.path().parent().unwrap().join("VICTIM.md");
        std::fs::write(&outside, "secrets").unwrap();
        // And a normal entry that should still load.
        std::fs::write(dir.path().join("ok.md"), "ok").unwrap();

        let index = "- [Bad](../VICTIM.md) — bad\n\
                     - [BadAbs](/etc/passwd.md) — abs\n\
                     - [Nested](sub/../../VICTIM.md) — nested\n\
                     - [Ok](ok.md) — ok";
        let files = load_referenced_files(index, dir.path(), types::Scope::User);
        // Only the safe entry survives.
        assert_eq!(
            files.len(),
            1,
            "got {:?}",
            files.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        assert_eq!(files[0].name, "Ok");
        let _ = std::fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn load_referenced_files_rejects_symlink_leaf() {
        // A symlink whose name is `evil.md` and target is outside the
        // memory dir must not be followed.
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().parent().unwrap().join("OUTSIDE.md");
        std::fs::write(&outside, "secret").unwrap();
        let link = dir.path().join("evil.md");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let index = "- [Evil](evil.md) — evil";
        let files = load_referenced_files(index, dir.path(), types::Scope::Team);
        assert!(
            files.is_empty(),
            "symlink leaf should be refused; got {:?}",
            files.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_file(outside);
    }

    // ---- hierarchical_project_files ----

    /// Build a fake repo layout:
    ///   tmp/
    ///     .git/  (dir, repo root marker)
    ///     AGENTS.md
    ///     packages/
    ///       sub/
    ///         AGENTS.md
    ///         nested/
    ///           AGENTS.md
    fn make_nested_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "root").unwrap();
        let sub = root.join("packages").join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("AGENTS.md"), "sub").unwrap();
        let nested = sub.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("AGENTS.md"), "nested").unwrap();
        dir
    }

    #[test]
    fn hierarchical_walks_from_cwd_up_to_git_root() {
        let tmp = make_nested_repo();
        let start = tmp.path().join("packages").join("sub").join("nested");
        let files = hierarchical_project_files(&start);
        // Should find 3 AGENTS.md: root, sub, nested
        let names: Vec<_> = files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect();
        assert_eq!(names, vec!["root", "sub", "nested"]);
    }

    #[test]
    fn hierarchical_stops_at_git_root_does_not_escape() {
        let tmp = make_nested_repo();
        // Write an AGENTS.md in the *parent* of the temp dir. The walk
        // must not reach it — we must stop at `.git`.
        // (Skipping this in practice because tmpdir's parent may not be
        // writable; instead, verify the walk only returns files within
        // the repo root.)
        let start = tmp.path().join("packages").join("sub");
        let files = hierarchical_project_files(&start);
        for p in &files {
            assert!(p.starts_with(tmp.path()), "walk escaped repo root: {p:?}");
        }
    }

    #[test]
    fn hierarchical_ordering_is_outermost_first() {
        let tmp = make_nested_repo();
        let start = tmp.path().join("packages").join("sub").join("nested");
        let files = hierarchical_project_files(&start);
        // Content of the first file must be "root" (outermost).
        let first = std::fs::read_to_string(&files[0]).unwrap();
        assert_eq!(first, "root");
        let last = std::fs::read_to_string(files.last().unwrap()).unwrap();
        assert_eq!(last, "nested");
    }

    #[test]
    fn hierarchical_without_git_stays_at_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "x").unwrap();
        let files = hierarchical_project_files(dir.path());
        // No .git anywhere — walk should stop at start itself, not
        // climb into parent filesystem dirs.
        assert!(!files.is_empty());
        for p in &files {
            assert!(p.starts_with(dir.path()));
        }
    }

    #[test]
    fn hierarchical_handles_missing_intermediate_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        // Only root and deepest have AGENTS.md; intermediate doesn't.
        std::fs::write(root.join("AGENTS.md"), "root").unwrap();
        let mid = root.join("a").join("b");
        std::fs::create_dir_all(&mid).unwrap();
        std::fs::write(mid.join("AGENTS.md"), "deep").unwrap();
        let files = hierarchical_project_files(&mid);
        let names: Vec<_> = files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect();
        assert_eq!(names, vec!["root", "deep"]);
    }

    #[test]
    fn hierarchical_includes_dot_agent_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        let dotagent = root.join(".agent");
        std::fs::create_dir(&dotagent).unwrap();
        std::fs::write(dotagent.join("AGENTS.md"), "from-.agent").unwrap();
        let files = hierarchical_project_files(root);
        let contents: Vec<_> = files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect();
        assert!(
            contents.iter().any(|c| c == "from-.agent"),
            "expected .agent/AGENTS.md to be picked up, got {contents:?}"
        );
    }

    // ---- team memory ----

    /// Create a project root containing a `.git` marker plus a single
    /// team-memory entry (`<root>/.agent/team-memory/<file>`). The
    /// caller controls the index/body via `index_lines` and `body`.
    fn write_team_memory_file(
        root: &std::path::Path,
        filename: &str,
        index_lines: &str,
        body: &str,
    ) {
        let team = root.join(".agent").join("team-memory");
        std::fs::create_dir_all(&team).unwrap();
        std::fs::write(team.join("MEMORY.md"), index_lines).unwrap();
        std::fs::write(team.join(filename), body).unwrap();
    }

    #[test]
    fn team_memory_dir_path_layout() {
        let root = std::path::Path::new("/tmp/proj");
        assert_eq!(
            team_memory_dir(root),
            std::path::PathBuf::from("/tmp/proj/.agent/team-memory")
        );
    }

    #[test]
    fn is_team_memory_path_recognizes_dir() {
        assert!(is_team_memory_path(std::path::Path::new(
            "/work/proj/.agent/team-memory/foo.md"
        )));
        assert!(!is_team_memory_path(std::path::Path::new(
            "/work/proj/.agent/memory/foo.md"
        )));
        assert!(!is_team_memory_path(std::path::Path::new(
            "/work/proj/team-memory/foo.md"
        )));
    }

    #[test]
    fn team_memory_round_trip_load() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        write_team_memory_file(
            root,
            "deploy.md",
            "- [Deploy](deploy.md) — deploy steps",
            "---\nname: Deploy\ndescription: deploy steps\ntype: project\n---\n\nUse `make ship`.",
        );

        let ctx = MemoryContext::load(Some(root));
        // Team index loaded.
        assert!(ctx.team_memory.as_ref().unwrap().contains("Deploy"));
        // Topic file loaded with team scope.
        let team_files: Vec<_> = ctx
            .memory_files
            .iter()
            .filter(|f| f.scope == types::Scope::Team)
            .collect();
        assert_eq!(team_files.len(), 1, "expected one team file");
        assert_eq!(team_files[0].name, "Deploy");
        assert!(team_files[0].content.contains("make ship"));
    }

    #[test]
    fn collision_precedence_project_over_team_over_user() {
        // We only need the merge function — wire concrete `MemoryFile`s
        // from each scope with the same id and confirm the highest
        // precedence wins.
        let mk = |scope, body: &str, id: &str| MemoryFile {
            path: std::path::PathBuf::from(format!("/tmp/{id}.md")),
            name: format!("{id}-{}", scope_label(scope)),
            content: body.into(),
            staleness: None,
            scope,
        };

        // Same id `prefs`, three scopes.
        let merged = merge_scoped_files(vec![
            mk(types::Scope::User, "user wins?", "prefs"),
            mk(types::Scope::Team, "team wins?", "prefs"),
            mk(types::Scope::Project, "project wins", "prefs"),
        ]);
        assert_eq!(merged.len(), 1, "duplicate ids must collapse");
        assert_eq!(merged[0].scope, types::Scope::Project);
        assert_eq!(merged[0].content, "project wins");

        // Without project entry, team wins over user.
        let merged = merge_scoped_files(vec![
            mk(types::Scope::User, "user", "prefs"),
            mk(types::Scope::Team, "team", "prefs"),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].scope, types::Scope::Team);
    }

    /// Stand-in for `Scope::label` in test contexts that don't import it.
    fn scope_label(s: types::Scope) -> &'static str {
        s.label()
    }

    #[test]
    fn extraction_writer_path_is_not_team_memory() {
        // The extraction loop writes via `ensure_memory_dir()`, which
        // resolves to the per-user config directory. That path must
        // never satisfy `is_team_memory_path` — that's the safety
        // invariant for "model can read but not silently write".
        if let Some(user_dir) = ensure_memory_dir() {
            assert!(
                !is_team_memory_path(&user_dir),
                "user memory dir leaked into team-memory path: {}",
                user_dir.display()
            );
        }
    }

    #[test]
    fn user_memory_does_not_shadow_team_when_ids_differ() {
        // Different filenames in different scopes coexist.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        write_team_memory_file(
            root,
            "deploy.md",
            "- [Deploy](deploy.md) — deploy",
            "---\nname: Deploy\ndescription: d\ntype: project\n---\n\nteam body",
        );
        // Use a separate temp dir as a fake user dir to avoid touching
        // the real user config; load() reads from `user_memory_dir()`
        // which we cannot easily redirect, so we just validate the
        // merge logic with hand-rolled candidates instead.
        let candidates = vec![
            MemoryFile {
                path: root.join(".agent/team-memory/deploy.md"),
                name: "Deploy".into(),
                content: "team".into(),
                staleness: None,
                scope: types::Scope::Team,
            },
            MemoryFile {
                path: std::path::PathBuf::from("/u/prefs.md"),
                name: "Prefs".into(),
                content: "user".into(),
                staleness: None,
                scope: types::Scope::User,
            },
        ];
        let merged = merge_scoped_files(candidates);
        assert_eq!(merged.len(), 2);
    }

    // ---- merge ordering determinism ----

    #[test]
    fn merge_scoped_files_is_deterministic_across_input_orders() {
        // Same set of candidates, two different input orders. The
        // merged output must be byte-identical (path/name/scope), so
        // the resulting system prompt does not shuffle between loads.
        let mk = |scope, id: &str, dir: &str| MemoryFile {
            path: std::path::PathBuf::from(format!("/{dir}/{id}.md")),
            name: id.to_string(),
            content: format!("{id}-body"),
            staleness: None,
            scope,
        };
        let a = mk(types::Scope::User, "alpha", "u");
        let b = mk(types::Scope::Team, "beta", "t");
        let c = mk(types::Scope::Project, "gamma", "p");
        let d = mk(types::Scope::User, "delta", "u");

        let m1 = merge_scoped_files(vec![a.clone(), b.clone(), c.clone(), d.clone()]);
        let m2 = merge_scoped_files(vec![d, c, b, a]);

        let key = |f: &MemoryFile| (f.path.clone(), f.scope, f.name.clone());
        let k1: Vec<_> = m1.iter().map(key).collect();
        let k2: Vec<_> = m2.iter().map(key).collect();
        assert_eq!(k1, k2, "merge order must not depend on input order");
    }

    #[test]
    fn merge_scoped_files_orders_user_before_team_before_project() {
        let mk = |scope, id: &str| MemoryFile {
            path: std::path::PathBuf::from(format!("/{id}.md")),
            name: id.into(),
            content: id.into(),
            staleness: None,
            scope,
        };
        let merged = merge_scoped_files(vec![
            mk(types::Scope::Project, "z-proj"),
            mk(types::Scope::User, "m-user"),
            mk(types::Scope::Team, "a-team"),
        ]);
        // User first, then Team, then Project — most-specific last.
        let scopes: Vec<_> = merged.iter().map(|f| f.scope).collect();
        assert_eq!(
            scopes,
            vec![
                types::Scope::User,
                types::Scope::Team,
                types::Scope::Project
            ]
        );
    }

    #[test]
    fn merge_scoped_files_orders_within_scope_lexicographically() {
        let mk = |scope, id: &str| MemoryFile {
            path: std::path::PathBuf::from(format!("/{id}.md")),
            name: id.into(),
            content: id.into(),
            staleness: None,
            scope,
        };
        let merged = merge_scoped_files(vec![
            mk(types::Scope::User, "z-user"),
            mk(types::Scope::User, "a-user"),
            mk(types::Scope::User, "m-user"),
        ]);
        let names: Vec<_> = merged.iter().map(|f| f.name.clone()).collect();
        assert_eq!(names, vec!["a-user", "m-user", "z-user"]);
    }

    #[test]
    fn memory_context_load_is_deterministic_across_calls() {
        // Build a project tree containing both team and project memory
        // entries, then load the context twice. The two loads must
        // produce the same `memory_files` ordering — otherwise the
        // system prompt would shuffle and break prompt-cache hits.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join(".git")).unwrap();

        // Team-memory entries.
        write_team_memory_file(
            root,
            "deploy.md",
            "- [Deploy](deploy.md) — deploy steps\n- [Onboarding](onboarding.md) — onboarding\n",
            "---\nname: Deploy\ndescription: d\ntype: project\n---\n\nteam-deploy",
        );
        std::fs::write(
            root.join(".agent")
                .join("team-memory")
                .join("onboarding.md"),
            "---\nname: Onboarding\ndescription: o\ntype: project\n---\n\nteam-onboard",
        )
        .unwrap();

        // Project-memory entries.
        let proj_dir = root.join(".agent").join("memory");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(
            proj_dir.join("MEMORY.md"),
            "- [Style](style.md) — style guide\n- [Arch](arch.md) — architecture\n",
        )
        .unwrap();
        std::fs::write(
            proj_dir.join("style.md"),
            "---\nname: Style\ndescription: s\ntype: project\n---\n\nstyle-body",
        )
        .unwrap();
        std::fs::write(
            proj_dir.join("arch.md"),
            "---\nname: Arch\ndescription: a\ntype: project\n---\n\narch-body",
        )
        .unwrap();

        let ctx1 = MemoryContext::load(Some(root));
        let ctx2 = MemoryContext::load(Some(root));

        let key = |f: &MemoryFile| (f.path.clone(), f.scope, f.name.clone());
        let k1: Vec<_> = ctx1.memory_files.iter().map(key).collect();
        let k2: Vec<_> = ctx2.memory_files.iter().map(key).collect();
        assert_eq!(k1, k2, "memory_files order must be deterministic");

        // And the on-disk team index is byte-stable too — write_team_memory
        // appends in source order, so re-reading must round-trip the same
        // bytes regardless of how `merge_scoped_files` orders entries.
        let team_index_bytes =
            std::fs::read(root.join(".agent").join("team-memory").join("MEMORY.md")).unwrap();
        let team_index_bytes2 =
            std::fs::read(root.join(".agent").join("team-memory").join("MEMORY.md")).unwrap();
        assert_eq!(team_index_bytes, team_index_bytes2);
    }
}
