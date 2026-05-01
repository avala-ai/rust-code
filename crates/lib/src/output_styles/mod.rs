//! Output-style loader.
//!
//! Output styles inject a short instruction block into the system
//! prompt that shapes the agent's voice. Built-in styles ship with
//! the binary (`default`, `concise`, `explanatory`, `learning`).
//!
//! Users can extend the set by dropping markdown files into either
//! of two directories:
//!
//! - `<project>/.agent/output-styles/*.md`
//! - `~/.config/agent-code/output-styles/*.md`
//!
//! Each markdown file uses YAML frontmatter:
//!
//! ```text
//! ---
//! name: friendly
//! description: Warm, conversational tone for pairing sessions.
//! applies_to:
//!   - main
//!   - subagent
//! ---
//!
//! Speak warmly. Use first-person plural. Acknowledge progress as
//! we go and call out anything ambiguous before acting on it.
//! ```
//!
//! - `name` (required) — the id used by `/output-style <name>`. On
//!   collision, the disk style wins over a built-in and a warning is
//!   logged.
//! - `description` (required) — one-line summary shown in the
//!   `/output-style` listing.
//! - `applies_to` (optional) — a list of subagent kinds the style
//!   applies to. When empty, the style applies to every kind.
//!
//! Malformed files (missing required fields, unclosed frontmatter,
//! unparseable YAML) are skipped with a warning so a single bad file
//! cannot crash session startup.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

/// Which agent in the runtime is consuming the output style.
///
/// The `applies_to` frontmatter list is matched against this so a
/// style authored for one role doesn't bleed into the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    /// The user-facing top-level agent (interactive REPL, `--prompt`
    /// one-shots run directly by a human).
    Main,
    /// A child agent spawned by the `Agent` tool.
    Subagent,
}

impl AgentKind {
    /// Canonical string id used in `applies_to` lists.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Subagent => "subagent",
        }
    }
}

/// Where an output style was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStyleSource {
    /// Hard-coded styles that ship with the binary.
    BuiltIn,
    /// Loaded from `<project>/.agent/output-styles/`.
    Project,
    /// Loaded from `~/.config/agent-code/output-styles/`.
    User,
}

impl OutputStyleSource {
    /// Short label shown in the `/output-style` listing.
    pub fn label(&self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

/// A single resolvable output style.
#[derive(Debug, Clone)]
pub struct OutputStyle {
    /// The style id used by `/output-style <name>`.
    pub name: String,
    /// One-line description shown in listings.
    pub description: String,
    /// Prompt fragment injected into the system prompt when this
    /// style is active. May be empty (e.g. the built-in `default`).
    pub body: String,
    /// Subagent kinds this style applies to. An empty list means the
    /// style applies to every kind.
    pub applies_to: Vec<String>,
    /// Where this style was loaded from.
    pub source: OutputStyleSource,
    /// The file the style was loaded from. `None` for built-ins.
    pub source_path: Option<PathBuf>,
    /// Stable digest of the rendered body. Hashed into the system-prompt
    /// cache key so editing a style file in-session and re-selecting the
    /// same id correctly invalidates the cached prompt.
    pub content_hash: [u8; 12],
}

impl OutputStyle {
    /// Whether this style should be active for the given agent role.
    /// An empty `applies_to` list means "all kinds".
    pub fn applies_to_kind(&self, kind: AgentKind) -> bool {
        self.applies_to.is_empty() || self.applies_to.iter().any(|k| k == kind.as_str())
    }
}

/// Whether `value` is a known `applies_to` kind. Case-sensitive on
/// purpose: the docs say lowercase, and accepting `Main` would force
/// every reader (including the matcher in `OutputStyle::applies_to_kind`)
/// to grow case-insensitive logic for no real benefit.
fn is_valid_applies_to(value: &str) -> bool {
    matches!(value, "main" | "subagent")
}

/// 12-byte SHA256 prefix of `body` — long enough to detect any real
/// edit, short enough to keep the cache key cheap.
fn body_digest(body: &str) -> [u8; 12] {
    let digest = Sha256::digest(body.as_bytes());
    let mut out = [0u8; 12];
    out.copy_from_slice(&digest[..12]);
    out
}

/// Frontmatter schema for disk-loaded styles.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OutputStyleFrontmatter {
    name: Option<String>,
    description: Option<String>,
    applies_to: Option<Vec<String>>,
}

/// Registry of built-in and disk-loaded output styles.
///
/// Load with [`OutputStyleRegistry::load_all`]. On id collision, the
/// disk style wins over the built-in and a warning is logged.
#[derive(Debug, Clone, Default)]
pub struct OutputStyleRegistry {
    styles: Vec<OutputStyle>,
}

impl OutputStyleRegistry {
    /// Empty registry. Useful for tests.
    pub fn new() -> Self {
        Self { styles: Vec::new() }
    }

    /// Load built-in styles plus disk styles from the project and
    /// user directories. Disk styles override built-ins by id; a
    /// warning is logged on collision.
    pub fn load_all(project_root: Option<&Path>) -> Self {
        Self::load_all_with_user_dir(project_root, user_output_styles_dir())
    }

    /// Variant of [`load_all`] that takes the user-level output-styles
    /// directory explicitly. Tests pass a tempdir here so the assertions
    /// don't depend on whatever happens to live in the developer's real
    /// `~/.config/agent-code/output-styles/`. Production code should
    /// keep using [`load_all`].
    ///
    /// `user_dir` of `None` skips the user lookup entirely.
    pub fn load_all_with_user_dir(project_root: Option<&Path>, user_dir: Option<PathBuf>) -> Self {
        let mut registry = Self::new();

        // Built-ins go in first so that disk-loaded styles can override
        // them by id below.
        registry.load_builtins();

        // Project-level styles take precedence over user-level styles
        // on collision (consistent with how skills resolve), so load
        // user first then project.
        if let Some(dir) = user_dir
            && dir.is_dir()
        {
            registry.load_from_dir(&dir, OutputStyleSource::User);
        }

        if let Some(root) = project_root {
            let project_dir = root.join(".agent").join("output-styles");
            if project_dir.is_dir() {
                registry.load_from_dir(&project_dir, OutputStyleSource::Project);
            }
        }

        debug!("Loaded {} output styles", registry.styles.len());
        registry
    }

    /// Add the styles that ship with the binary. These mirror the
    /// hardcoded `ResponseStyle` enum so existing call sites keep
    /// working without a disk file.
    fn load_builtins(&mut self) {
        let entries: &[(&str, &str, &str)] = &[
            ("default", "No override (the codebase's default voice).", ""),
            (
                "concise",
                "Shorter responses with fewer qualifiers.",
                "Prefer shorter responses with fewer qualifiers. Skip prefaces and \
                 recaps. Report results directly. When a short answer suffices, use \
                 one.",
            ),
            (
                "explanatory",
                "Explain reasoning and trade-offs as you go.",
                "Explain your reasoning as you go. When a decision has alternatives, \
                 briefly note the trade-off you considered and why the chosen path \
                 wins. Prioritise clarity over brevity.",
            ),
            (
                "learning",
                "Narrate steps for users new to the codebase.",
                "You are pair-programming with someone new to this codebase. Before \
                 each significant edit or tool call, narrate what you're about to do \
                 and why in plain language. Favour explanation over terseness, but \
                 keep it focused on the task at hand.",
            ),
        ];

        for (name, description, body) in entries {
            self.styles.push(OutputStyle {
                name: (*name).to_string(),
                description: (*description).to_string(),
                body: (*body).to_string(),
                applies_to: Vec::new(),
                source: OutputStyleSource::BuiltIn,
                source_path: None,
                content_hash: body_digest(body),
            });
        }
    }

    /// Read every `*.md` file in `dir`, parse it, and merge it into
    /// the registry. Disk styles override existing entries with the
    /// same id and log a warning so the user knows their file is
    /// shadowing a built-in (or another disk file).
    fn load_from_dir(&mut self, dir: &Path, source: OutputStyleSource) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    "Failed to read output-styles directory {}: {e}",
                    dir.display()
                );
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|e| e != "md") {
                continue;
            }

            match load_style_file(&path, source) {
                Ok(style) => {
                    if let Some(existing) = self.styles.iter().position(|s| s.name == style.name) {
                        warn!(
                            "Output style '{}' from {} overrides {} entry",
                            style.name,
                            path.display(),
                            self.styles[existing].source.label()
                        );
                        self.styles[existing] = style;
                    } else {
                        self.styles.push(style);
                    }
                }
                Err(e) => {
                    warn!("Failed to load output style {}: {e}", path.display());
                }
            }
        }
    }

    /// Find a style by id (case-sensitive — ids are user-controlled
    /// filenames or frontmatter names, not human input).
    pub fn find(&self, name: &str) -> Option<&OutputStyle> {
        self.styles.iter().find(|s| s.name == name)
    }

    /// Iterate every loaded style.
    pub fn all(&self) -> &[OutputStyle] {
        &self.styles
    }

    /// Number of loaded styles.
    pub fn len(&self) -> usize {
        self.styles.len()
    }
}

/// Read a single style file. Returns an error string suitable for the
/// `warn!` log line on failure; the caller skips the file but keeps
/// loading.
fn load_style_file(path: &Path, source: OutputStyleSource) -> Result<OutputStyle, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    let (frontmatter, body) = parse_frontmatter(&content)?;

    let name = frontmatter
        .name
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing required `name` field in frontmatter".to_string())?;

    let description = frontmatter
        .description
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing required `description` field in frontmatter".to_string())?;

    // Reject unknown `applies_to` values at parse time. Without this a
    // typo like `Main` or `mian` loads silently, applies to nothing,
    // and the user has no signal that their style is dead. The set of
    // valid kinds is the canonical lowercase ids from
    // `AgentKind::as_str()` — keep it in lockstep with that enum.
    let applies_to: Vec<String> = frontmatter
        .applies_to
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .collect();

    for entry in &applies_to {
        if !is_valid_applies_to(entry) {
            return Err(format!(
                "invalid `applies_to` value {entry:?} — expected one of \
                 \"main\" or \"subagent\" (lowercase)"
            ));
        }
    }

    let body = body.trim().to_string();
    let content_hash = body_digest(&body);
    Ok(OutputStyle {
        name,
        description,
        body,
        applies_to,
        source,
        source_path: Some(path.to_path_buf()),
        content_hash,
    })
}

/// Parse YAML frontmatter from a markdown document.
///
/// We avoid pulling in a full YAML parser dependency. The supported
/// surface is intentionally small but enough for the schema:
///
/// - `key: value` (string)
/// - `key:` followed by indented `- item` lines (list of strings)
/// - `key: [a, b, c]` (inline list of strings)
///
/// Anything more exotic is reported as an error so the file is
/// skipped rather than silently misinterpreted.
fn parse_frontmatter(content: &str) -> Result<(OutputStyleFrontmatter, String), String> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Err(
            "no YAML frontmatter — expected a `---` block with `name` and `description`"
                .to_string(),
        );
    }

    let after_first = &trimmed[3..];
    // The opening `---` has to be followed by a newline; otherwise it
    // is more likely a horizontal rule than the start of frontmatter.
    let after_first = after_first
        .strip_prefix('\n')
        .or_else(|| after_first.strip_prefix("\r\n"))
        .ok_or_else(|| "frontmatter opener `---` must be on its own line".to_string())?;

    let closing = after_first
        .find("\n---")
        .ok_or_else(|| "frontmatter not closed (missing closing `---`)".to_string())?;

    let yaml = &after_first[..closing];
    // Skip over the `\n---` plus any trailing newline on that line.
    let mut body_start = closing + 4;
    if let Some(rest) = after_first.get(body_start..) {
        if let Some(stripped) = rest.strip_prefix("\r\n") {
            body_start += rest.len() - stripped.len();
        } else if rest.starts_with('\n') {
            body_start += 1;
        }
    }
    let body = after_first.get(body_start..).unwrap_or("").to_string();

    let frontmatter = parse_yaml(yaml)?;
    Ok((frontmatter, body))
}

/// Mini YAML parser that handles the subset we document.
fn parse_yaml(src: &str) -> Result<OutputStyleFrontmatter, String> {
    let mut out = OutputStyleFrontmatter::default();

    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim_end();
        let trimmed = line.trim();

        // Blank lines and comments are ignored.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }

        // Top-level keys must not be indented.
        if line.starts_with([' ', '\t']) {
            return Err(format!(
                "unexpected indented line at the top level: {trimmed:?}"
            ));
        }

        let (key, rest) = trimmed
            .split_once(':')
            .ok_or_else(|| format!("expected `key: value`, got {trimmed:?}"))?;

        let key = key.trim();
        let value = rest.trim();

        match key {
            "name" => out.name = Some(parse_scalar(value)?),
            "description" => out.description = Some(parse_scalar(value)?),
            "applies_to" => {
                if value.is_empty() {
                    // Block-style list — collect indented `- item` lines.
                    let mut items = Vec::new();
                    let mut j = i + 1;
                    while j < lines.len() {
                        let next = lines[j];
                        let next_trim = next.trim();
                        if next_trim.is_empty() || next_trim.starts_with('#') {
                            j += 1;
                            continue;
                        }
                        if !next.starts_with([' ', '\t']) {
                            break;
                        }
                        let item = next_trim
                            .strip_prefix('-')
                            .ok_or_else(|| {
                                format!(
                                    "expected list item starting with `-` for `applies_to`, \
                                     got {next_trim:?}"
                                )
                            })?
                            .trim();
                        if !item.is_empty() {
                            items.push(parse_scalar(item)?);
                        }
                        j += 1;
                    }
                    out.applies_to = Some(items);
                    i = j;
                    continue;
                } else {
                    // Inline list `[a, b, c]` — also accept a single
                    // bare scalar so `applies_to: main` does the right
                    // thing.
                    out.applies_to = Some(parse_inline_list(value)?);
                }
            }
            other => {
                // Unknown keys are tolerated so we can extend the
                // schema without breaking older binaries.
                debug!("ignoring unknown output-style frontmatter key: {other}");
            }
        }
        i += 1;
    }

    Ok(out)
}

/// Strip surrounding single or double quotes from a YAML scalar.
fn parse_scalar(value: &str) -> Result<String, String> {
    let v = value.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        Ok(v[1..v.len() - 1].to_string())
    } else {
        Ok(v.to_string())
    }
}

/// Parse `[a, b, c]` (inline list) or a single bare scalar.
fn parse_inline_list(value: &str) -> Result<Vec<String>, String> {
    let v = value.trim();
    if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if inner.trim().is_empty() {
            return Ok(Vec::new());
        }
        let items = inner
            .split(',')
            .map(|s| parse_scalar(s.trim()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items.into_iter().filter(|s| !s.is_empty()).collect())
    } else {
        // Bare scalar — treat as a single-item list.
        Ok(vec![parse_scalar(v)?])
    }
}

/// User-level output-styles directory: `~/.config/agent-code/output-styles/`
/// on Linux/macOS, the platform-equivalent on Windows.
///
/// The `AGENT_CODE_USER_OUTPUT_STYLES_DIR` env var overrides the
/// resolved path. Tests use this to point at a tempdir so assertions
/// don't depend on the developer's real config dir.
fn user_output_styles_dir() -> Option<PathBuf> {
    if let Ok(override_dir) = std::env::var("AGENT_CODE_USER_OUTPUT_STYLES_DIR") {
        let trimmed = override_dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::config_dir().map(|d| d.join("agent-code").join("output-styles"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn builtins_are_loaded_when_no_disk_dirs_present() {
        let mut registry = OutputStyleRegistry::new();
        registry.load_builtins();
        assert!(registry.find("default").is_some());
        assert!(registry.find("concise").is_some());
        assert!(registry.find("explanatory").is_some());
        assert!(registry.find("learning").is_some());
        assert_eq!(
            registry.find("default").unwrap().source,
            OutputStyleSource::BuiltIn
        );
    }

    #[test]
    fn parses_frontmatter_with_block_list() {
        let content = "---\n\
                       name: friendly\n\
                       description: Warm tone\n\
                       applies_to:\n  \
                       - main\n  \
                       - subagent\n\
                       ---\n\
                       Body text here.";
        let (fm, body) = parse_frontmatter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("friendly"));
        assert_eq!(fm.description.as_deref(), Some("Warm tone"));
        assert_eq!(
            fm.applies_to.unwrap(),
            vec!["main".to_string(), "subagent".to_string()]
        );
        assert_eq!(body.trim(), "Body text here.");
    }

    #[test]
    fn parses_frontmatter_with_inline_list() {
        let content = "---\n\
                       name: x\n\
                       description: y\n\
                       applies_to: [a, b, c]\n\
                       ---\n\
                       body";
        let (fm, _) = parse_frontmatter(content).unwrap();
        assert_eq!(
            fm.applies_to.unwrap(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn parses_frontmatter_with_quoted_scalars() {
        let content = "---\n\
                       name: \"quoted name\"\n\
                       description: 'single quoted'\n\
                       ---\n\
                       body";
        let (fm, _) = parse_frontmatter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("quoted name"));
        assert_eq!(fm.description.as_deref(), Some("single quoted"));
    }

    #[test]
    fn missing_frontmatter_returns_error() {
        let err = parse_frontmatter("just a body, no frontmatter").unwrap_err();
        assert!(err.contains("frontmatter"));
    }

    #[test]
    fn unclosed_frontmatter_returns_error() {
        let err = parse_frontmatter("---\nname: oops\nno closer here").unwrap_err();
        assert!(err.contains("not closed"));
    }

    #[test]
    fn unknown_frontmatter_keys_are_ignored() {
        let content = "---\n\
                       name: x\n\
                       description: y\n\
                       experimental_thing: 42\n\
                       ---\n\
                       body";
        let (fm, _) = parse_frontmatter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("x"));
        assert_eq!(fm.description.as_deref(), Some("y"));
    }

    #[test]
    fn applies_to_kind_default_matches_everything() {
        let style = OutputStyle {
            name: "n".into(),
            description: "d".into(),
            body: String::new(),
            applies_to: Vec::new(),
            source: OutputStyleSource::BuiltIn,
            source_path: None,
            content_hash: body_digest(""),
        };
        assert!(style.applies_to_kind(AgentKind::Main));
        assert!(style.applies_to_kind(AgentKind::Subagent));
    }

    #[test]
    fn applies_to_kind_respects_explicit_list() {
        let style = OutputStyle {
            name: "n".into(),
            description: "d".into(),
            body: String::new(),
            applies_to: vec!["main".into()],
            source: OutputStyleSource::BuiltIn,
            source_path: None,
            content_hash: body_digest(""),
        };
        assert!(style.applies_to_kind(AgentKind::Main));
        assert!(!style.applies_to_kind(AgentKind::Subagent));
    }

    #[test]
    fn content_hash_changes_when_body_changes() {
        // The cache key in `query::run_loop` depends on this digest, so
        // any body edit must produce a different hash. A regression here
        // would cause stale system prompts after `/reload`.
        assert_ne!(body_digest("alpha"), body_digest("beta"));
        assert_eq!(body_digest("alpha"), body_digest("alpha"));
    }

    #[test]
    fn output_style_load_from_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let styles_dir = project_root.join(".agent").join("output-styles");
        std::fs::create_dir_all(&styles_dir).unwrap();

        write_file(
            &styles_dir,
            "friendly.md",
            "---\n\
             name: friendly\n\
             description: Warm tone for pairing\n\
             applies_to:\n  - main\n\
             ---\n\
             Speak warmly. Acknowledge ambiguity.",
        );

        let registry = OutputStyleRegistry::load_all_with_user_dir(Some(project_root), None);
        let style = registry
            .find("friendly")
            .expect("project style should be picked up");
        assert_eq!(style.source, OutputStyleSource::Project);
        assert_eq!(style.description, "Warm tone for pairing");
        assert_eq!(style.applies_to, vec!["main".to_string()]);
        assert!(style.body.contains("Speak warmly"));
        assert!(style.applies_to_kind(AgentKind::Main));
        assert!(!style.applies_to_kind(AgentKind::Subagent));
    }

    #[test]
    fn project_style_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let styles_dir = project_root.join(".agent").join("output-styles");
        std::fs::create_dir_all(&styles_dir).unwrap();

        write_file(
            &styles_dir,
            "concise.md",
            "---\n\
             name: concise\n\
             description: Project override of concise\n\
             ---\n\
             Custom concise prompt.",
        );

        let registry = OutputStyleRegistry::load_all_with_user_dir(Some(project_root), None);
        let style = registry.find("concise").expect("style should exist");
        assert_eq!(style.source, OutputStyleSource::Project);
        assert_eq!(style.description, "Project override of concise");
        assert_eq!(style.body, "Custom concise prompt.");

        // The other built-ins are still around.
        assert_eq!(
            registry.find("default").unwrap().source,
            OutputStyleSource::BuiltIn
        );
    }

    #[test]
    fn malformed_frontmatter_skips_file_without_crashing() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let styles_dir = project_root.join(".agent").join("output-styles");
        std::fs::create_dir_all(&styles_dir).unwrap();

        // No closing fence — should be skipped.
        write_file(
            &styles_dir,
            "broken.md",
            "---\nname: broken\ndescription: oops\n\nbody without closer",
        );

        // Missing required `name`.
        write_file(
            &styles_dir,
            "no-name.md",
            "---\n\
             description: missing the name\n\
             ---\n\
             body",
        );

        // Missing required `description`.
        write_file(
            &styles_dir,
            "no-desc.md",
            "---\n\
             name: nodesc\n\
             ---\n\
             body",
        );

        // A valid file alongside the broken ones — proves the loader
        // didn't bail out at the first error.
        write_file(
            &styles_dir,
            "good.md",
            "---\n\
             name: good\n\
             description: a clean style\n\
             ---\n\
             body",
        );

        let registry = OutputStyleRegistry::load_all_with_user_dir(Some(project_root), None);
        assert!(registry.find("good").is_some(), "valid file must load");
        assert!(
            registry.find("broken").is_none(),
            "unclosed frontmatter must be skipped"
        );
        assert!(
            registry.find("nodesc").is_none(),
            "missing description must be skipped"
        );
        // Built-ins must still be present.
        assert!(registry.find("default").is_some());
    }

    #[test]
    fn invalid_applies_to_value_rejects_file() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let styles_dir = project_root.join(".agent").join("output-styles");
        std::fs::create_dir_all(&styles_dir).unwrap();

        // Typo — "mian" instead of "main".
        write_file(
            &styles_dir,
            "typo.md",
            "---\n\
             name: typo\n\
             description: typo style\n\
             applies_to: [mian]\n\
             ---\n\
             body",
        );

        // Wrong case — docs specify lowercase.
        write_file(
            &styles_dir,
            "case.md",
            "---\n\
             name: case\n\
             description: capitalised\n\
             applies_to: [Main]\n\
             ---\n\
             body",
        );

        // Valid neighbour — proves the loader keeps going.
        write_file(
            &styles_dir,
            "ok.md",
            "---\n\
             name: ok\n\
             description: clean\n\
             applies_to: [main, subagent]\n\
             ---\n\
             body",
        );

        let registry = OutputStyleRegistry::load_all_with_user_dir(Some(project_root), None);
        assert!(
            registry.find("typo").is_none(),
            "unknown applies_to value must be rejected at parse time"
        );
        assert!(
            registry.find("case").is_none(),
            "non-canonical case must be rejected — docs say lowercase"
        );
        assert!(
            registry.find("ok").is_some(),
            "valid neighbouring style must still load"
        );
    }

    #[test]
    fn applies_to_omitted_is_still_valid() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let styles_dir = project_root.join(".agent").join("output-styles");
        std::fs::create_dir_all(&styles_dir).unwrap();

        write_file(
            &styles_dir,
            "all.md",
            "---\n\
             name: all\n\
             description: applies to everything\n\
             ---\n\
             body",
        );

        let registry = OutputStyleRegistry::load_all_with_user_dir(Some(project_root), None);
        let style = registry.find("all").expect("style should load");
        assert!(style.applies_to.is_empty());
        assert!(style.applies_to_kind(AgentKind::Main));
        assert!(style.applies_to_kind(AgentKind::Subagent));
    }

    #[test]
    fn non_md_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let styles_dir = project_root.join(".agent").join("output-styles");
        std::fs::create_dir_all(&styles_dir).unwrap();
        write_file(&styles_dir, "notes.txt", "name: ignored");

        let registry = OutputStyleRegistry::load_all_with_user_dir(Some(project_root), None);
        assert!(registry.find("ignored").is_none());
    }
}
