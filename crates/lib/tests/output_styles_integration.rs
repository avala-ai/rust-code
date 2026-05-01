//! Integration tests for disk-loaded output styles.
//!
//! These tests prove the end-to-end loading path: a markdown file
//! placed in a fake `<project>/.agent/output-styles/` directory is
//! discovered, parsed, and merged with the built-in styles, with
//! disk entries winning on id collision.

use std::fs;
use std::path::{Path, PathBuf};

use agent_code_lib::output_styles::{AgentKind, OutputStyleRegistry, OutputStyleSource};

/// Empty tempdir to feed `load_all_with_user_dir` as the user-level
/// output-styles directory. The integration test must NOT pick up
/// whatever happens to live in the developer's real
/// `~/.config/agent-code/output-styles/`.
fn empty_user_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir for hermetic user dir")
}

fn user_dir_path(td: &tempfile::TempDir) -> Option<PathBuf> {
    Some(td.path().to_path_buf())
}

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
    let user = empty_user_dir();
    let registry = OutputStyleRegistry::load_all_with_user_dir(None, user_dir_path(&user));
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
    let user = empty_user_dir();

    let registry =
        OutputStyleRegistry::load_all_with_user_dir(Some(tempdir.path()), user_dir_path(&user));

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
    let user = empty_user_dir();

    let registry =
        OutputStyleRegistry::load_all_with_user_dir(Some(tempdir.path()), user_dir_path(&user));

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
    let user = empty_user_dir();

    let registry =
        OutputStyleRegistry::load_all_with_user_dir(Some(tempdir.path()), user_dir_path(&user));

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
    let user = empty_user_dir();

    fs::write(
        &path,
        "---\n\
         name: custom\n\
         description: Initial body\n\
         ---\n\
         First version of the prompt body.",
    )
    .unwrap();
    let first =
        OutputStyleRegistry::load_all_with_user_dir(Some(tempdir.path()), user_dir_path(&user));
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
    let second =
        OutputStyleRegistry::load_all_with_user_dir(Some(tempdir.path()), user_dir_path(&user));
    let hash_after = second.find("custom").unwrap().content_hash;

    assert_ne!(
        hash_before, hash_after,
        "editing the body must produce a different content_hash"
    );
}

#[test]
fn user_directory_path_is_isolated_in_tests() {
    // Pin the user-level dir to an empty tempdir so this test cannot
    // pick up whatever may live in `~/.config/agent-code/output-styles/`
    // on the developer's machine. Without this, a developer who happens
    // to have a custom `concise.md` in their real user dir would see
    // its source rather than the fixture's source and fail the test.
    let tempdir = tempfile::tempdir().unwrap();
    stage_fixtures_into_project(tempdir.path());
    let user = empty_user_dir();

    let registry =
        OutputStyleRegistry::load_all_with_user_dir(Some(tempdir.path()), user_dir_path(&user));
    let concise = registry.find("concise").unwrap();
    assert_eq!(
        concise.source,
        OutputStyleSource::Project,
        "project dir must beat both built-ins and user dir"
    );
}

/// Hermeticity guard: dropping a uniquely-named style into the
/// explicit `user_dir` must surface as a `User`-sourced entry, while
/// leaving the user dir empty must mean no user-sourced styles. If
/// `load_all_with_user_dir` regressed to silently reading the real
/// `~/.config/agent-code/output-styles/`, the second half of this
/// test would falsely pick up developer-local styles.
#[test]
fn explicit_user_dir_is_actually_used_and_isolated() {
    let user_td = tempfile::tempdir().unwrap();
    let user_dir = user_td.path();
    fs::write(
        user_dir.join("hermetic-marker.md"),
        "---\n\
         name: hermetic-marker\n\
         description: only exists in the explicit user dir\n\
         ---\n\
         body",
    )
    .unwrap();

    let project_td = tempfile::tempdir().unwrap();
    let registry = OutputStyleRegistry::load_all_with_user_dir(
        Some(project_td.path()),
        Some(user_dir.to_path_buf()),
    );
    let marker = registry
        .find("hermetic-marker")
        .expect("explicit user dir entry must be loaded");
    assert_eq!(marker.source, OutputStyleSource::User);

    // Now point at an empty user dir and confirm that no `User`-sourced
    // styles leak in. If the loader silently fell back to the real
    // `~/.config/agent-code/output-styles/`, this assertion would flake
    // on developer machines that have any user-level styles configured.
    let empty_user = tempfile::tempdir().unwrap();
    let registry = OutputStyleRegistry::load_all_with_user_dir(
        Some(project_td.path()),
        Some(empty_user.path().to_path_buf()),
    );
    assert!(
        registry
            .all()
            .iter()
            .all(|s| s.source != OutputStyleSource::User),
        "no User-sourced styles must appear when user dir is empty"
    );
}
