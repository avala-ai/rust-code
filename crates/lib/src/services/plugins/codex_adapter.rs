//! Loader for plugins authored in the codex-style format.
//!
//! Detection: a TOML file with at least one top-level
//! `[mcp_servers.<name>]` table. Each entry becomes a synthetic
//! [`PluginManifest`] with a single MCP-server contribution and a
//! synthetic id of `codex-mcp-<server-name>`.
//!
//! When the loader is invoked against a directory (e.g. the user's
//! `~/.codex` home), it also picks up sibling `prompts/*.md` files as
//! slash commands, mirroring the codex convention of putting prompt
//! markdown next to the config.
//!
//! The detection rule is intentionally narrow: a TOML file with no
//! `mcp_servers.*` tables is **not** considered a codex plugin. This
//! keeps the loader from accidentally hijacking arbitrary TOML files
//! (notably our own `.agent/settings.toml` or any random project
//! config).

use std::path::{Path, PathBuf};

use tracing::warn;

use super::{Plugin, PluginCommand, PluginFormat, PluginManifest, PluginMcpServer, PluginSource};

/// Try to parse a directory as a codex-style plugin. Returns `None`
/// if no TOML file in the directory declares `[mcp_servers.*]`.
///
/// Used by the registry as a last-resort fallback when neither
/// `plugin.toml` nor `.claude-plugin/plugin.json` is present.
pub fn try_load_dir(dir: &Path, source: PluginSource) -> Option<Plugin> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        if let Some(plugin) = parse_codex_toml(&p, dir, source.clone()) {
            return Some(plugin);
        }
    }
    None
}

/// Discover synthetic codex plugins from the user's `~/.codex` home.
///
/// Each `[mcp_servers.<name>]` table in `~/.codex/config.toml` becomes
/// its own synthetic plugin (`codex-mcp-<name>`) so it shows up
/// independently in `/plugin list`. Prompts in `~/.codex/prompts/`
/// are bundled into a single companion plugin
/// (`codex-prompts`) so they're discoverable without polluting the
/// per-server listing.
pub fn load_codex_home(codex_root: &Path) -> Vec<Plugin> {
    let mut out = Vec::new();

    let config_toml = codex_root.join("config.toml");
    if config_toml.is_file() {
        out.extend(parse_each_mcp_server(&config_toml, codex_root));
    }

    let prompts_dir = codex_root.join("prompts");
    if prompts_dir.is_dir() {
        let commands = read_prompt_dir(&prompts_dir);
        if !commands.is_empty() {
            let manifest = PluginManifest {
                name: "codex-prompts".into(),
                description: Some(format!(
                    "Slash commands sourced from {}",
                    prompts_dir.display()
                )),
                format: PluginFormat::Codex,
                commands: commands.iter().map(|c| c.name.clone()).collect(),
                ..Default::default()
            };
            out.push(Plugin {
                manifest,
                source: PluginSource::SiblingTool(prompts_dir.clone()),
                path: prompts_dir,
                command_bodies: commands,
            });
        }
    }

    out
}

/// Single-file form: scan a TOML file and, if it has at least one
/// `[mcp_servers.<name>]` table, return a single synthetic plugin
/// covering all servers and any sibling prompts/ directory.
fn parse_codex_toml(toml_path: &Path, dir: &Path, source: PluginSource) -> Option<Plugin> {
    let raw = std::fs::read_to_string(toml_path).ok()?;
    let value: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            warn!("codex-style parse error in {}: {e}", toml_path.display());
            return None;
        }
    };

    let servers = extract_mcp_servers(&value)?;
    if servers.is_empty() {
        return None;
    }

    let mut manifest = PluginManifest {
        name: dir
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| format!("codex-mcp-{s}"))
            .unwrap_or_else(|| "codex-mcp".to_string()),
        format: PluginFormat::Codex,
        mcp_servers: servers,
        ..Default::default()
    };

    let mut command_bodies = Vec::new();
    let prompts_dir = dir.join("prompts");
    if prompts_dir.is_dir() {
        for cmd in read_prompt_dir(&prompts_dir) {
            manifest.commands.push(cmd.name.clone());
            command_bodies.push(cmd);
        }
    }

    Some(Plugin {
        manifest,
        source,
        path: dir.to_path_buf(),
        command_bodies,
    })
}

/// `~/.codex/config.toml` form: emit one synthetic plugin per declared
/// server.
fn parse_each_mcp_server(toml_path: &Path, codex_root: &Path) -> Vec<Plugin> {
    let raw = match std::fs::read_to_string(toml_path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let value: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            warn!("codex-style parse error in {}: {e}", toml_path.display());
            return Vec::new();
        }
    };
    let servers = match extract_mcp_servers(&value) {
        Some(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };

    servers
        .into_iter()
        .map(|server| {
            let manifest = PluginManifest {
                name: format!("codex-mcp-{}", server.name),
                description: Some(format!(
                    "MCP server '{}' discovered from {}",
                    server.name,
                    toml_path.display()
                )),
                format: PluginFormat::Codex,
                mcp_servers: vec![server],
                ..Default::default()
            };
            Plugin {
                manifest,
                source: PluginSource::SiblingTool(toml_path.to_path_buf()),
                path: codex_root.to_path_buf(),
                command_bodies: Vec::new(),
            }
        })
        .collect()
}

fn extract_mcp_servers(value: &toml::Value) -> Option<Vec<PluginMcpServer>> {
    let table = value.get("mcp_servers")?.as_table()?;
    let mut out = Vec::new();
    for (name, entry) in table {
        let entry_table = match entry.as_table() {
            Some(t) => t,
            None => continue,
        };
        let command = entry_table
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let url = entry_table
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let args = entry_table
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let env = entry_table
            .get("env")
            .and_then(|v| v.as_table())
            .map(|t| {
                t.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        out.push(PluginMcpServer {
            name: name.clone(),
            command,
            args,
            url,
            env,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}

fn read_prompt_dir(dir: &Path) -> Vec<PluginCommand> {
    let mut out: Vec<PluginCommand> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let p: PathBuf = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body = match std::fs::read_to_string(&p) {
            Ok(b) => b,
            Err(e) => {
                warn!("prompt read {}: {e}", p.display());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mcp_servers_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.echo]
command = "echo-mcp"
args = ["--stdio"]
"#,
        )
        .unwrap();

        let plugin = parse_codex_toml(&path, dir.path(), PluginSource::User).unwrap();
        assert_eq!(plugin.manifest.format, PluginFormat::Codex);
        assert_eq!(plugin.manifest.mcp_servers.len(), 1);
        assert_eq!(plugin.manifest.mcp_servers[0].name, "echo");
    }

    #[test]
    fn rejects_toml_without_mcp_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[api]\nmodel = \"gpt-5\"\n").unwrap();
        assert!(parse_codex_toml(&path, dir.path(), PluginSource::User).is_none());
    }

    #[test]
    fn picks_up_sibling_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[mcp_servers.echo]\ncommand = \"echo-mcp\"\n").unwrap();
        std::fs::create_dir(dir.path().join("prompts")).unwrap();
        std::fs::write(dir.path().join("prompts").join("hi.md"), "Say hi.").unwrap();

        let plugin = parse_codex_toml(&path, dir.path(), PluginSource::User).unwrap();
        assert_eq!(plugin.manifest.commands, vec!["hi".to_string()]);
        assert_eq!(plugin.command_bodies.len(), 1);
    }

    #[test]
    fn home_loader_emits_one_plugin_per_server() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
[mcp_servers.alpha]
command = "alpha"

[mcp_servers.beta]
command = "beta"
"#,
        )
        .unwrap();
        let plugins = load_codex_home(dir.path());
        let names: Vec<&str> = plugins.iter().map(|p| p.manifest.name.as_str()).collect();
        assert!(names.contains(&"codex-mcp-alpha"));
        assert!(names.contains(&"codex-mcp-beta"));
    }
}
