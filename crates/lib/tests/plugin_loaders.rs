//! End-to-end test for the multi-format plugin loader.
//!
//! Sets up a temp project tree with one plugin per supported format
//! (native, claude-style, codex-style) and asserts each is recognised
//! with the expected source/format labels and contributed surface.

use std::path::{Path, PathBuf};

use agent_code_lib::services::plugins::{PluginFormat, PluginRegistry, PluginSource};

/// Path to the on-disk fixture set bundled with this test file.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("plugins")
}

/// Stage all three fixture plugins under a temp project's
/// `.agent/plugins/` and run the loader against it.
#[test]
fn loader_recognises_all_three_formats_side_by_side() {
    let project = tempfile::tempdir().unwrap();
    let plugins_dir = project.path().join(".agent").join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();

    for name in ["native-foo", "claude-bar", "codex-baz"] {
        let src = fixtures().join(name);
        let dst = plugins_dir.join(name);
        copy_dir(&src, &dst);
    }

    let registry = PluginRegistry::load_all_with_compat(Some(project.path()), false);

    let native = registry.find("native-foo").expect("native plugin missing");
    assert_eq!(native.manifest.format, PluginFormat::Native);
    assert_eq!(native.source, PluginSource::Project);
    assert_eq!(native.manifest.skills, vec!["foo".to_string()]);
    assert_eq!(native.manifest.hooks.len(), 1);

    let claude = registry.find("claude-bar").expect("claude plugin missing");
    assert_eq!(claude.manifest.format, PluginFormat::Claude);
    assert_eq!(claude.source, PluginSource::Project);
    assert!(claude.manifest.commands.contains(&"hello".to_string()));
    assert!(claude.manifest.skills.contains(&"baz".to_string()));
    assert_eq!(claude.manifest.mcp_servers.len(), 1);
    assert_eq!(claude.manifest.mcp_servers[0].name, "claude-echo");

    // codex-baz directory has only `config.toml` + `prompts/`. The
    // native loader skips it (no plugin.toml); the codex fallback
    // detects the [mcp_servers.echo] table and synthesises a manifest.
    let codex = registry
        .all()
        .iter()
        .find(|p| p.manifest.format == PluginFormat::Codex)
        .expect("codex plugin missing");
    assert_eq!(codex.source, PluginSource::Project);
    assert!(codex.manifest.name.starts_with("codex-mcp-"));
    assert_eq!(codex.manifest.mcp_servers.len(), 1);
    assert_eq!(codex.manifest.mcp_servers[0].name, "echo");
    assert!(codex.manifest.commands.contains(&"echo".to_string()));
}

#[test]
fn enable_disable_overlay_flips_state() {
    let project = tempfile::tempdir().unwrap();
    let plugins_dir = project.path().join(".agent").join("plugins");
    std::fs::create_dir_all(&plugins_dir).unwrap();
    copy_dir(
        &fixtures().join("native-foo"),
        &plugins_dir.join("native-foo"),
    );

    // Default: enabled.
    let r1 = PluginRegistry::load_all(Some(project.path()));
    assert!(r1.find("native-foo").unwrap().is_enabled());

    // Disable via the per-project overlay file.
    let overlay = project.path().join(".agent").join("plugin-settings.toml");
    let mut s = agent_code_lib::services::plugins::settings::PluginSettings::default();
    s.set_enabled("native-foo", false);
    s.save(&overlay).unwrap();

    let r2 = PluginRegistry::load_all(Some(project.path()));
    assert!(!r2.find("native-foo").unwrap().is_enabled());
    // skill_dirs / hooks / mcp_servers must skip disabled plugins.
    assert!(r2.skill_dirs().is_empty());
    assert!(r2.hooks().is_empty());
}

#[test]
fn codex_loader_rejects_toml_without_mcp_servers() {
    // A directory with a non-codex TOML file must NOT be picked up
    // as a synthetic plugin. This covers the "don't accidentally
    // hijack settings.toml" rule.
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join(".agent").join("plugins").join("benign");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("settings.toml"),
        "[api]\nmodel = \"gpt-5\"\n",
    )
    .unwrap();
    let registry = PluginRegistry::load_all(Some(dir.path()));
    assert!(registry.find("benign").is_none());
    // No format, no source — nothing was registered.
    assert_eq!(
        registry
            .all()
            .iter()
            .filter(|p| p.path.starts_with(&plugin_dir))
            .count(),
        0
    );
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}
