//! Integration tests for disk-loaded output styles.
//!
//! These tests prove the end-to-end loading path: a markdown file
//! placed in a fake `<project>/.agent/output-styles/` directory is
//! discovered, parsed, and merged with the built-in styles, with
//! disk entries winning on id collision.

use std::fs;
use std::path::Path;

use agent_code_lib::output_styles::{AgentKind, OutputStyleRegistry, OutputStyleSource};

/// Copy every fixture under `crates/lib/tests/fixtures/output_styles/`
/// into a fake `<project>/.agent/output-styles/` directory inside a
/// fresh tempdir. Returns the project root.
fn stage_fixtures_into_project(tempdir: &Path) {
    let dest = tempdir.join(".agent").join("output-styles");
    fs::create_dir_all(&dest).unwrap();

    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("output_styles");

    for entry in fs::read_dir(&src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let target = dest.join(path.file_name().unwrap());
        fs::copy(&path, &target).unwrap();
    }
}

#[test]
fn builtins_load_with_no_project_dir() {
    let registry = OutputStyleRegistry::load_all(None);
    for name in ["default", "concise", "explanatory", "learning"] {
        let style = registry
            .find(name)
            .unwrap_or_else(|| panic!("built-in style '{name}' should exist"));
        assert_eq!(style.source, OutputStyleSource::BuiltIn);
    }
}

#[test]
fn project_directory_styles_are_picked_up() {
    let tempdir = tempfile::tempdir().unwrap();
    stage_fixtures_into_project(tempdir.path());

    let registry = OutputStyleRegistry::load_all(Some(tempdir.path()));

    // The "friendly" fixture is unique to the project dir.
    let friendly = registry
        .find("friendly")
        .expect("project-level style 'friendly' should be discovered end-to-end");
    assert_eq!(friendly.source, OutputStyleSource::Project);
    assert_eq!(
        friendly.description,
        "Warm, conversational tone for pairing sessions."
    );
    assert_eq!(
        friendly.applies_to,
        vec!["main".to_string(), "subagent".to_string()]
    );
    assert!(
        friendly.body.contains("Speak warmly"),
        "body must come from the markdown file, got: {:?}",
        friendly.body
    );
    assert!(friendly.applies_to_kind(AgentKind::Main));
    assert!(friendly.applies_to_kind(AgentKind::Subagent));
}

#[test]
fn disk_style_overrides_builtin_with_same_id() {
    let tempdir = tempfile::tempdir().unwrap();
    stage_fixtures_into_project(tempdir.path());

    let registry = OutputStyleRegistry::load_all(Some(tempdir.path()));

    // The "concise" fixture has the same id as a built-in. The disk
    // entry must win.
    let concise = registry.find("concise").unwrap();
    assert_eq!(concise.source, OutputStyleSource::Project);
    assert_eq!(
        concise.description,
        "Project-level override of the built-in concise style."
    );
    assert!(concise.body.contains("ruthless about brevity"));

    // Other built-ins are still around with their original source.
    assert_eq!(
        registry.find("default").unwrap().source,
        OutputStyleSource::BuiltIn
    );
    assert_eq!(
        registry.find("explanatory").unwrap().source,
        OutputStyleSource::BuiltIn
    );
}

#[test]
fn malformed_files_are_skipped_without_crashing() {
    let tempdir = tempfile::tempdir().unwrap();
    stage_fixtures_into_project(tempdir.path());

    let registry = OutputStyleRegistry::load_all(Some(tempdir.path()));

    // The "broken" fixture has an unclosed frontmatter — it must be
    // silently skipped, but the rest of the registry must still load.
    assert!(
        registry.find("broken").is_none(),
        "malformed fixture must not be registered"
    );
    assert!(
        registry.find("friendly").is_some(),
        "neighbouring valid fixture must still load"
    );
    assert!(
        registry.find("default").is_some(),
        "built-ins must still be present after a parse failure"
    );
}

/// Regression for the codex finding: editing a style file in-session
/// must change its `content_hash`, otherwise the system-prompt cache
/// (which mixes the active style's content hash into its key) would
/// stay stale after a `/reload`.
#[test]
fn editing_disk_style_body_changes_content_hash() {
    let tempdir = tempfile::tempdir().unwrap();
    let styles_dir = tempdir.path().join(".agent").join("output-styles");
    fs::create_dir_all(&styles_dir).unwrap();
    let path = styles_dir.join("custom.md");

    fs::write(
        &path,
        "---\n\
         name: custom\n\
         description: Initial body\n\
         ---\n\
         First version of the prompt body.",
    )
    .unwrap();
    let first = OutputStyleRegistry::load_all(Some(tempdir.path()));
    let hash_before = first.find("custom").unwrap().content_hash;

    fs::write(
        &path,
        "---\n\
         name: custom\n\
         description: Initial body\n\
         ---\n\
         Second version of the prompt body — totally different.",
    )
    .unwrap();
    let second = OutputStyleRegistry::load_all(Some(tempdir.path()));
    let hash_after = second.find("custom").unwrap().content_hash;

    assert_ne!(
        hash_before, hash_after,
        "editing the body must produce a different content_hash"
    );
}

#[test]
fn user_directory_path_is_isolated_in_tests() {
    // The loader prefers project entries on collision, so a fixture
    // copied into the project dir always wins regardless of what may
    // (or may not) exist in `~/.config/agent-code/output-styles/`.
    // This test guards against accidentally polluting the assertions
    // with the developer's real user-level styles.
    let tempdir = tempfile::tempdir().unwrap();
    stage_fixtures_into_project(tempdir.path());

    let registry = OutputStyleRegistry::load_all(Some(tempdir.path()));
    let concise = registry.find("concise").unwrap();
    assert_eq!(
        concise.source,
        OutputStyleSource::Project,
        "project dir must beat both built-ins and user dir"
    );
}
