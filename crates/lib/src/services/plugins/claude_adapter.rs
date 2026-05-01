//! Loader for plugins authored in the claude-style format.
//!
//! Detection: a directory containing `.claude-plugin/plugin.json`.
//!
//! The adapter reads the JSON manifest plus sibling conventional
//! directories (`commands/`, `agents/`, `skills/`, `hooks/`) and an
//! optional `.mcp.json` file, then projects the result onto a
//! [`PluginManifest`]. The rest of the system never sees the
//! adapter-specific layout — only the normalised manifest.
//!
//! All conversions are lossy in one direction: anything we don't
//! understand is dropped silently with a warning, never copied
//! verbatim. The aim is "load the obvious surface" rather than
//! "round-trip every field".

use std::path::Path;

use serde::Deserialize;
use tracing::warn;

use super::{
    Plugin, PluginCommand, PluginFormat, PluginHook, PluginManifest, PluginMcpServer, PluginSource,
};

/// Subset of the JSON manifest we recognise. Unknown fields are
/// ignored; we never round-trip the original document, so dropping
/// them is safe.
#[derive(Debug, Deserialize, Default)]
struct ClaudePluginJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<serde_json::Value>,
}

/// `.mcp.json` shape used by claude-style plugins. Mirrors the
/// codex-style `[mcp_servers.<name>]` table content but in JSON.
#[derive(Debug, Deserialize, Default)]
struct ClaudeMcpJson {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: std::collections::HashMap<String, ClaudeMcpEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct ClaudeMcpEntry {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
}

/// `hooks/*.json` and `hooks/hooks.json` shape. Each entry maps an
/// event name to a command. The adapter normalises this into our
/// internal `PluginHook` shape.
#[derive(Debug, Deserialize, Default)]
struct ClaudeHooksJson {
    #[serde(default)]
    hooks: Vec<ClaudeHookEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct ClaudeHookEntry {
    event: String,
    command: String,
    #[serde(default, rename = "toolName", alias = "tool_name")]
    tool_name: Option<String>,
}

/// `marketplace.json` listing multiple plugins inside a single
/// directory. Each entry points at a sub-directory that itself looks
/// like a claude-style plugin.
#[derive(Debug, Deserialize, Default)]
struct ClaudeMarketplaceJson {
    #[serde(default)]
    plugins: Vec<ClaudeMarketplaceEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct ClaudeMarketplaceEntry {
    #[serde(default)]
    path: Option<String>,
}

/// Parse a single claude-style plugin directory.
pub fn load_plugin_dir(path: &Path, source: PluginSource) -> Result<Plugin, String> {
    let manifest_path = path.join(".claude-plugin").join("plugin.json");
    let content =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("read manifest: {e}"))?;
    let parsed: ClaudePluginJson =
        serde_json::from_str(&content).map_err(|e| format!("parse manifest: {e}"))?;

    let name = parsed.name.clone().unwrap_or_else(|| {
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    let author = parsed.author.as_ref().and_then(extract_author);

    let mut manifest = PluginManifest {
        name,
        version: parsed.version,
        description: parsed.description,
        author,
        format: PluginFormat::Claude,
        ..Default::default()
    };

    // Slash commands from commands/*.md.
    let mut command_bodies: Vec<PluginCommand> = Vec::new();
    let commands_dir = path.join("commands");
    if commands_dir.is_dir() {
        for cmd in read_markdown_dir(&commands_dir, "command") {
            manifest.commands.push(cmd.name.clone());
            command_bodies.push(cmd);
        }
    }

    // Agents/*.md become "agent prompt commands" — surfaced under
    // /plugin list as a command; the actual subagent wiring is the
    // job of the existing agent system, which already loads from
    // disk. Recording them here gives users visibility.
    let agents_dir = path.join("agents");
    if agents_dir.is_dir() {
        for cmd in read_markdown_dir(&agents_dir, "agent") {
            // Surface agent prompts under a stable namespace so they
            // don't collide with regular commands.
            let agent_name = format!("agent:{}", cmd.name);
            manifest.commands.push(agent_name.clone());
            command_bodies.push(PluginCommand {
                name: agent_name,
                body: cmd.body,
                source_path: cmd.source_path,
            });
        }
    }

    // Skills/*.md — record names so they show in /skills via the
    // existing skill loader (which scans plugin skill_dirs()).
    let skills_dir = path.join("skills");
    if skills_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&skills_dir)
    {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("md")
                && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
            {
                manifest.skills.push(stem.to_string());
            }
        }
    }

    // Hooks: `hooks/hooks.json` (preferred) or any `hooks/*.json`.
    let hooks_dir = path.join("hooks");
    if hooks_dir.is_dir() {
        let preferred = hooks_dir.join("hooks.json");
        if preferred.is_file() {
            collect_hooks_from(&preferred, &mut manifest.hooks);
        } else if let Ok(entries) = std::fs::read_dir(&hooks_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("json") {
                    collect_hooks_from(&p, &mut manifest.hooks);
                }
            }
        }
    }

    // Optional .mcp.json — read at load time, never written back.
    let mcp_path = path.join(".mcp.json");
    if mcp_path.is_file() {
        match std::fs::read_to_string(&mcp_path) {
            Ok(raw) => match serde_json::from_str::<ClaudeMcpJson>(&raw) {
                Ok(parsed) => {
                    for (server_name, entry) in parsed.mcp_servers {
                        manifest.mcp_servers.push(PluginMcpServer {
                            name: server_name,
                            command: entry.command,
                            args: entry.args,
                            url: entry.url,
                            env: entry.env,
                        });
                    }
                }
                Err(e) => warn!(".mcp.json parse error in {}: {e}", path.display()),
            },
            Err(e) => warn!(".mcp.json read error in {}: {e}", path.display()),
        }
    }

    Ok(Plugin {
        manifest,
        source,
        path: path.to_path_buf(),
        command_bodies,
    })
}

/// Iterate marketplace entries inside a claude-style marketplace
/// bundle. Each `plugins[].path` is resolved relative to the
/// marketplace root and parsed as an individual plugin directory.
pub fn load_marketplace_dir(root: &Path, source: PluginSource) -> Vec<Plugin> {
    let manifest_path = root.join(".claude-plugin").join("marketplace.json");
    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let parsed: ClaudeMarketplaceJson = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            warn!("marketplace.json parse error in {}: {e}", root.display());
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for entry in parsed.plugins {
        let Some(rel) = entry.path else { continue };
        let plugin_root = root.join(&rel);
        if !plugin_root.is_dir() {
            warn!("marketplace entry path missing: {}", plugin_root.display());
            continue;
        }
        match load_plugin_dir(&plugin_root, source.clone()) {
            Ok(p) => out.push(p),
            Err(e) => warn!(
                "marketplace plugin failed at {}: {e}",
                plugin_root.display()
            ),
        }
    }
    out
}

fn read_markdown_dir(dir: &Path, _kind: &str) -> Vec<PluginCommand> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body = match std::fs::read_to_string(&p) {
            Ok(b) => b,
            Err(e) => {
                warn!("read {}: {e}", p.display());
                continue;
            }
        };
        out.push(PluginCommand {
            name: stem.to_string(),
            body,
            source_path: p,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn collect_hooks_from(path: &Path, out: &mut Vec<PluginHook>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            warn!("hook read {}: {e}", path.display());
            return;
        }
    };
    // Accept either {hooks: [...]} or a bare array of hook entries.
    if let Ok(parsed) = serde_json::from_str::<ClaudeHooksJson>(&raw) {
        for entry in parsed.hooks {
            out.push(PluginHook {
                event: entry.event,
                command: entry.command,
                tool_name: entry.tool_name,
            });
        }
        return;
    }
    if let Ok(entries) = serde_json::from_str::<Vec<ClaudeHookEntry>>(&raw) {
        for entry in entries {
            out.push(PluginHook {
                event: entry.event,
                command: entry.command,
                tool_name: entry.tool_name,
            });
        }
        return;
    }
    warn!("hooks file unparseable: {}", path.display());
}

fn extract_author(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => map
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_minimal_claude_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("p");
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(plugin_dir.join("commands")).unwrap();
        std::fs::write(
            plugin_dir.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"hello","version":"0.1.0","description":"hi"}"#,
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join("commands").join("greet.md"),
            "Greet the user.",
        )
        .unwrap();

        let plugin = load_plugin_dir(&plugin_dir, PluginSource::User).unwrap();
        assert_eq!(plugin.manifest.name, "hello");
        assert_eq!(plugin.manifest.format, PluginFormat::Claude);
        assert!(plugin.manifest.commands.contains(&"greet".to_string()));
        assert_eq!(plugin.command_bodies.len(), 1);
        assert_eq!(plugin.command_bodies[0].body, "Greet the user.");
    }

    #[test]
    fn parses_mcp_json() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("p");
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            plugin_dir.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"with-mcp"}"#,
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join(".mcp.json"),
            r#"{"mcpServers":{"echo":{"command":"echo-mcp","args":["--stdio"]}}}"#,
        )
        .unwrap();
        let plugin = load_plugin_dir(&plugin_dir, PluginSource::User).unwrap();
        assert_eq!(plugin.manifest.mcp_servers.len(), 1);
        assert_eq!(plugin.manifest.mcp_servers[0].name, "echo");
        assert_eq!(
            plugin.manifest.mcp_servers[0].command.as_deref(),
            Some("echo-mcp")
        );
    }

    #[test]
    fn missing_mcp_json_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("p");
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            plugin_dir.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"plain"}"#,
        )
        .unwrap();
        let plugin = load_plugin_dir(&plugin_dir, PluginSource::User).unwrap();
        assert!(plugin.manifest.mcp_servers.is_empty());
    }
}
