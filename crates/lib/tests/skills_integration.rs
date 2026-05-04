//! Integration tests for the skill system.
//!
//! Tests skill loading from directories, frontmatter parsing,
//! argument expansion, and bundled skill availability.

use std::io::Write;

use agent_code_lib::skills::SkillRegistry;

#[test]
fn bundled_skills_load_without_project_dir() {
    let registry = SkillRegistry::load_bundled_only();
    assert!(
        registry.all().len() >= 12,
        "Expected at least 12 bundled skills, got {}",
        registry.all().len()
    );
}

#[test]
fn bundled_skills_are_all_invocable() {
    let registry = SkillRegistry::load_bundled_only();
    let invocable = registry.user_invocable();
    assert!(
        invocable.len() >= 12,
        "Expected at least 12 invocable skills, got {}",
        invocable.len()
    );
}

#[test]
fn find_bundled_skill_by_name() {
    let registry = SkillRegistry::load_bundled_only();
    for name in [
        "commit",
        "review",
        "test",
        "explain",
        "debug",
        "pr",
        "refactor",
        "init",
        "security-review",
        "advisor",
        "bughunter",
        "plan",
    ] {
        assert!(
            registry.find(name).is_some(),
            "Bundled skill '{name}' not found"
        );
    }
}

#[test]
fn load_skill_from_temp_directory() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".agent").join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    let skill_path = skills_dir.join("my-skill.md");
    let mut f = std::fs::File::create(&skill_path).unwrap();
    writeln!(
        f,
        "---\ndescription: A test skill\nuserInvocable: true\n---\n\nDo something with {{{{arg}}}}."
    )
    .unwrap();

    let registry = SkillRegistry::load_all(Some(dir.path()));
    let skill = registry.find("my-skill");
    assert!(skill.is_some(), "Custom skill 'my-skill' not loaded");

    let skill = skill.unwrap();
    assert_eq!(skill.metadata.description.as_deref(), Some("A test skill"));
    assert!(skill.metadata.user_invocable);

    let expanded = skill.expand(Some("main.rs"));
    assert!(expanded.contains("main.rs"));
}

#[test]
fn project_skill_overrides_bundled() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".agent").join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Create a skill with the same name as a bundled one.
    let skill_path = skills_dir.join("commit.md");
    let mut f = std::fs::File::create(&skill_path).unwrap();
    writeln!(
        f,
        "---\ndescription: Custom commit workflow\nuserInvocable: true\n---\n\nCustom commit."
    )
    .unwrap();

    let registry = SkillRegistry::load_all(Some(dir.path()));
    let skill = registry.find("commit").unwrap();
    assert_eq!(
        skill.metadata.description.as_deref(),
        Some("Custom commit workflow"),
        "Project skill should override bundled"
    );
}

#[test]
fn bundled_skill_batch_invokes_with_expected_prompt() {
    let registry = SkillRegistry::load_bundled_only();
    let skill = registry.find("batch").expect("batch should be bundled");
    let body = skill.expand(None);
    assert!(body.contains("Resolve the target set"));
    assert!(body.contains("STOP for confirmation"));
    assert!(body.to_lowercase().contains("you're done"));
}

#[test]
fn bundled_skill_loop_invokes_with_expected_prompt() {
    let registry = SkillRegistry::load_bundled_only();
    let skill = registry.find("loop").expect("loop should be bundled");
    let body = skill.expand(None);
    assert!(body.contains("exit condition"));
    assert!(body.contains("ceiling"));
    assert!(body.to_lowercase().contains("prompt-only"));
}

#[test]
fn bundled_skill_remember_invokes_with_expected_prompt() {
    let registry = SkillRegistry::load_bundled_only();
    let skill = registry
        .find("remember")
        .expect("remember should be bundled");
    let body = skill.expand(None);
    assert!(body.contains("Classify the type"));
    assert!(body.contains("Pick the scope"));
    assert!(body.contains("STOP and confirm"));
}

#[test]
fn bundled_skill_simplify_invokes_with_expected_prompt() {
    let registry = SkillRegistry::load_bundled_only();
    let skill = registry
        .find("simplify")
        .expect("simplify should be bundled");
    let body = skill.expand(None);
    assert!(body.contains("Read the diff"));
    assert!(body.contains("dead weight"));
    assert!(body.contains("STOP for confirmation"));
}

#[test]
fn bundled_skill_stuck_invokes_with_expected_prompt() {
    let registry = SkillRegistry::load_bundled_only();
    let skill = registry.find("stuck").expect("stuck should be bundled");
    let body = skill.expand(None);
    assert!(body.contains("Reconstruct what was tried"));
    assert!(body.contains("3 alternative approaches"));
    assert!(body.contains("not \"give up\""));
}

#[test]
fn bundled_skill_verify_invokes_with_expected_prompt() {
    let registry = SkillRegistry::load_bundled_only();
    let skill = registry.find("verify").expect("verify should be bundled");
    let body = skill.expand(None);
    assert!(body.contains("State the claim"));
    assert!(body.contains("blast radius"));
    assert!(body.contains("does NOT auto-fix"));
}

#[test]
fn bundled_skill_app_builder_invokes_with_expected_prompt() {
    let registry = SkillRegistry::load_bundled_only();
    let skill = registry
        .find("app-builder")
        .expect("app-builder should be bundled");
    let body = skill.expand(None);
    assert!(body.contains("Clarify the brief"));
    assert!(body.contains("Pick the stack"));
    assert!(body.to_lowercase().contains("prompt-only"));
}

#[test]
fn phase_8_3_skills_all_present() {
    let registry = SkillRegistry::load_bundled_only();
    for name in [
        "batch",
        "loop",
        "remember",
        "simplify",
        "stuck",
        "verify",
        "app-builder",
    ] {
        assert!(
            registry.find(name).is_some(),
            "Phase 8.3 bundled skill '{name}' not found in registry"
        );
    }
}

#[test]
fn directory_skill_with_skill_md() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".agent").join("skills");
    let skill_subdir = skills_dir.join("deploy");
    std::fs::create_dir_all(&skill_subdir).unwrap();

    let skill_path = skill_subdir.join("SKILL.md");
    let mut f = std::fs::File::create(&skill_path).unwrap();
    writeln!(
        f,
        "---\ndescription: Deploy to production\nuserInvocable: true\n---\n\nDeploy steps."
    )
    .unwrap();

    let registry = SkillRegistry::load_all(Some(dir.path()));
    let skill = registry.find("deploy");
    assert!(skill.is_some(), "Directory skill 'deploy' not loaded");
    assert_eq!(
        skill.unwrap().metadata.description.as_deref(),
        Some("Deploy to production")
    );
}
