//! Plugin system.
//!
//! Plugins bundle skills, slash commands, hooks, and configuration
//! together as installable packages. The loader recognises three on-disk
//! formats and normalises each into a single internal representation:
//!
//! - **Native**: `<plugin>/plugin.toml` with `skills/`, `hooks/`, `bin/`.
//! - **Claude-style**: `<plugin>/.claude-plugin/plugin.json` with
//!   `commands/`, `agents/`, `skills/`, `hooks/`, optional `.mcp.json`.
//! - **Codex-style**: a TOML file declaring at least one
//!   `[mcp_servers.<name>]` table, optionally next to a sibling
//!   `prompts/` directory of slash-command markdown files.
//!
//! The marketplace MVP only loads from disk; URL-based fetching is
//! stubbed and surfaced as "not yet implemented" through the
//! `/plugin install` command. The architecture deliberately keeps the
//! adapter modules independent so a marketplace fetcher can be a
//! drop-in addition later.
//!
//! Plugins are loaded from `~/.config/agent-code/plugins/` and
//! project-level `.agent/plugins/`. Codex-style plugins additionally
//! discover `~/.codex/config.toml` plus `~/.codex/prompts/` when the
//! caller opts in via [`PluginRegistry::load_all_with_compat`].

pub mod claude_adapter;
pub mod codex_adapter;
pub mod settings;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// On-disk format the plugin was authored in. Drives both loader
/// selection and the source label shown in `/plugin list`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    #[default]
    Native,
    Claude,
    Codex,
}

impl PluginFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Claude => "claude-compat",
            Self::Codex => "codex-compat",
        }
    }
}

/// Where the loaded plugin came from. Distinct from [`PluginFormat`]:
/// a single plugin can be authored in any format and live at any
/// source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// Compiled into the binary via the static built-in registry.
    Builtin,
    /// User-level on-disk install (`~/.config/agent-code/plugins/`).
    User,
    /// Project-level on-disk install (`<project>/.agent/plugins/`).
    Project,
    /// Discovered from a sibling tool's config (e.g. `~/.codex/`).
    /// The path is the file or directory the entry was synthesised
    /// from, recorded for transparency in `/plugin list`.
    SiblingTool(PathBuf),
}

impl PluginSource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
            Self::SiblingTool(_) => "sibling",
        }
    }
}

/// Plugin metadata. The native form is parsed directly from
/// `plugin.toml`; the claude / codex adapters synthesise this same
/// struct so the rest of the system stays format-agnostic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    /// Source format the plugin was authored in. Defaulted at parse
    /// time for native manifests; explicitly set by the adapters.
    #[serde(default)]
    pub format: PluginFormat,
    /// Skill file basenames provided by this plugin.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Slash-command names contributed by this plugin.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Hook definitions.
    #[serde(default)]
    pub hooks: Vec<PluginHook>,
    /// MCP server contributions. These are surfaced in `/plugin list`
    /// but never written back to the user's on-disk config.
    #[serde(default)]
    pub mcp_servers: Vec<PluginMcpServer>,
    /// Configuration keys this plugin adds.
    #[serde(default)]
    pub config: std::collections::HashMap<String, serde_json::Value>,
    /// Whether the plugin is enabled. Persisted in
    /// `<project>/.agent/plugin-settings.toml`; absent on first load.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// A hook defined by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHook {
    pub event: String,
    pub command: String,
    #[serde(default)]
    pub tool_name: Option<String>,
}

/// A plugin-contributed MCP server registration. Adapter-derived; not
/// persisted to the user's MCP config — only added to the in-memory
/// runtime view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginMcpServer {
    pub name: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

/// A slash-command body sourced from a markdown file inside a claude
/// or codex plugin. Stored separately from the manifest so adapters
/// can surface the prompt template without parsing it again.
#[derive(Debug, Clone)]
pub struct PluginCommand {
    pub name: String,
    pub body: String,
    pub source_path: PathBuf,
}

/// A loaded plugin with its manifest, source, and on-disk path.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: PluginManifest,
    pub source: PluginSource,
    /// On-disk root of the plugin, or the synthetic root for
    /// builtin/sibling-tool entries.
    pub path: PathBuf,
    /// Slash-command markdown bodies sourced from `commands/` or
    /// `prompts/`. Populated by the claude / codex adapters; empty for
    /// native plugins (which use the existing skill loader instead).
    pub command_bodies: Vec<PluginCommand>,
}

impl Plugin {
    /// Stable identifier shown in slash commands. Builtins are
    /// prefixed with `@builtin/`; everything else uses the manifest
    /// name verbatim.
    pub fn id(&self) -> String {
        match self.source {
            PluginSource::Builtin => format!("@builtin/{}", self.manifest.name),
            _ => self.manifest.name.clone(),
        }
    }

    /// True when the plugin is enabled, accounting for the
    /// `plugin-settings.toml` overlay.
    pub fn is_enabled(&self) -> bool {
        self.manifest.enabled.unwrap_or(true)
    }
}

/// A built-in plugin compiled into the binary. The slot is wired now
/// but the slice is empty: bundling actual built-in plugins is a
/// follow-up.
pub struct BuiltinPlugin {
    pub id: &'static str,
    pub manifest: fn() -> PluginManifest,
}

/// The compiled-in built-in registry. Kept empty intentionally for
/// this MVP — slot exists so adding builtins is a one-line drop-in.
pub static BUILTIN_PLUGINS: &[BuiltinPlugin] = &[];

/// Registry of loaded plugins.
pub struct PluginRegistry {
    plugins: Vec<Plugin>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Load plugins from all configured directories using the native
    /// loader only. Kept for backwards compatibility with existing
    /// callers that pre-date the multi-format work.
    pub fn load_all(project_root: Option<&Path>) -> Self {
        Self::load_all_with_compat(project_root, false)
    }

    /// Load plugins from all sources, optionally including the
    /// claude-style and codex-style compatibility loaders.
    ///
    /// Source ordering is `builtin → user → project → sibling-tool`,
    /// which mirrors the user-visible `/plugin list` order. Each
    /// directory walk picks the loader that matches the entry's
    /// on-disk shape: a `.claude-plugin/plugin.json` selects the
    /// claude adapter, a TOML file with at least one
    /// `[mcp_servers.<name>]` table selects the codex adapter, and
    /// anything else falls back to the native loader.
    pub fn load_all_with_compat(project_root: Option<&Path>, include_codex_compat: bool) -> Self {
        let mut registry = Self::new();

        // 0. Built-in plugins (compiled in).
        for entry in BUILTIN_PLUGINS {
            let manifest = (entry.manifest)();
            registry.plugins.push(Plugin {
                manifest,
                source: PluginSource::Builtin,
                path: PathBuf::from(format!("@builtin/{}", entry.id)),
                command_bodies: Vec::new(),
            });
        }

        // 1. User-level plugins.
        if let Some(dir) = user_plugin_dir() {
            registry.load_from_dir(&dir, PluginSource::User);
        }

        // 2. Project-level plugins.
        if let Some(root) = project_root {
            registry.load_from_dir(&root.join(".agent").join("plugins"), PluginSource::Project);
        }

        // 3. Codex-style sibling-tool compatibility (opt-in, but
        //    defaults on for first run via the caller).
        if include_codex_compat && let Some(home) = dirs::home_dir() {
            let codex_root = home.join(".codex");
            if codex_root.is_dir() {
                let synthetic = codex_adapter::load_codex_home(&codex_root);
                for plugin in synthetic {
                    registry.plugins.push(plugin);
                }
            }
        }

        // Apply enable/disable overlay from plugin-settings.toml.
        if let Some(root) = project_root {
            let overlay_path = root.join(".agent").join("plugin-settings.toml");
            if let Ok(overlay) = settings::PluginSettings::load(&overlay_path) {
                for plugin in &mut registry.plugins {
                    if let Some(enabled) = overlay.enabled_for(&plugin.id()) {
                        plugin.manifest.enabled = Some(enabled);
                    }
                }
            }
        }

        debug!("Loaded {} plugins", registry.plugins.len());
        registry
    }

    fn load_from_dir(&mut self, dir: &Path, source: PluginSource) {
        if !dir.is_dir() {
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            match load_one_plugin(&path, &source) {
                Ok(plugin) => {
                    debug!(
                        "Loaded plugin '{}' ({}/{}) from {}",
                        plugin.manifest.name,
                        plugin.source.label(),
                        plugin.manifest.format.label(),
                        path.display()
                    );
                    self.plugins.push(plugin);
                }
                Err(e) => {
                    warn!("Failed to load plugin at {}: {e}", path.display());
                }
            }
        }
    }

    /// Get all loaded plugins (regardless of enabled state).
    pub fn all(&self) -> &[Plugin] {
        &self.plugins
    }

    /// Plugins that are currently enabled.
    pub fn enabled(&self) -> Vec<&Plugin> {
        self.plugins.iter().filter(|p| p.is_enabled()).collect()
    }

    /// Find a plugin by name or `@builtin/<name>` id.
    pub fn find(&self, id: &str) -> Option<&Plugin> {
        self.plugins
            .iter()
            .find(|p| p.id() == id || p.manifest.name == id)
    }

    /// Get all skill directories from enabled plugins.
    pub fn skill_dirs(&self) -> Vec<PathBuf> {
        self.enabled()
            .into_iter()
            .map(|p| p.path.join("skills"))
            .filter(|d| d.is_dir())
            .collect()
    }

    /// Get all hook definitions from enabled plugins.
    pub fn hooks(&self) -> Vec<&PluginHook> {
        self.plugins
            .iter()
            .filter(|p| p.is_enabled())
            .flat_map(|p| &p.manifest.hooks)
            .collect()
    }

    /// Get all MCP server registrations contributed by enabled
    /// plugins. The runtime layer merges these into the active MCP
    /// config without writing them to disk.
    pub fn mcp_servers(&self) -> Vec<&PluginMcpServer> {
        self.plugins
            .iter()
            .filter(|p| p.is_enabled())
            .flat_map(|p| &p.manifest.mcp_servers)
            .collect()
    }

    /// Discover all executable tools from plugin bin/ directories.
    pub fn executable_tools(&self) -> Vec<crate::tools::plugin_exec::PluginExecTool> {
        self.plugins
            .iter()
            .filter(|p| p.is_enabled())
            .flat_map(|p| {
                crate::tools::plugin_exec::discover_plugin_executables(&p.path, &p.manifest.name)
            })
            .collect()
    }
}

/// Pick a loader based on the on-disk shape of `path` and call it.
fn load_one_plugin(path: &Path, source: &PluginSource) -> Result<Plugin, String> {
    let claude_manifest = path.join(".claude-plugin").join("plugin.json");
    if claude_manifest.is_file() {
        return claude_adapter::load_plugin_dir(path, source.clone());
    }

    let toml_path = path.join("plugin.toml");
    if toml_path.is_file() {
        return load_native_plugin(path, source.clone());
    }

    // Fall back to scanning a single TOML file with codex-style mcp
    // server tables. Most directory plugins use plugin.toml, so this
    // is a rare path.
    if let Some(synthetic) = codex_adapter::try_load_dir(path, source.clone()) {
        return Ok(synthetic);
    }

    Err(format!(
        "no recognised plugin manifest in {}",
        path.display()
    ))
}

fn load_native_plugin(path: &Path, source: PluginSource) -> Result<Plugin, String> {
    let manifest_path = path.join("plugin.toml");
    let content =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("Read error: {e}"))?;

    let mut manifest: PluginManifest =
        toml::from_str(&content).map_err(|e| format!("Parse error: {e}"))?;
    manifest.format = PluginFormat::Native;

    Ok(Plugin {
        manifest,
        source,
        path: path.to_path_buf(),
        command_bodies: Vec::new(),
    })
}

fn user_plugin_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("plugins"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry() {
        let reg = PluginRegistry::new();
        assert!(reg.all().is_empty());
        assert!(reg.enabled().is_empty());
    }

    #[test]
    fn native_plugin_loads_with_default_format() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join(".agent").join("plugins").join("foo");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            r#"name = "foo"
version = "0.1.0"
description = "Native test plugin"
skills = ["bar"]
"#,
        )
        .unwrap();

        let reg = PluginRegistry::load_all(Some(dir.path()));
        let p = reg.find("foo").unwrap();
        assert_eq!(p.manifest.name, "foo");
        assert_eq!(p.manifest.format, PluginFormat::Native);
        assert_eq!(p.source, PluginSource::Project);
        assert!(p.is_enabled());
    }

    #[test]
    fn id_uses_builtin_prefix_for_builtin_source() {
        let plugin = Plugin {
            manifest: PluginManifest {
                name: "demo".into(),
                ..Default::default()
            },
            source: PluginSource::Builtin,
            path: PathBuf::from("@builtin/demo"),
            command_bodies: Vec::new(),
        };
        assert_eq!(plugin.id(), "@builtin/demo");
    }
}
