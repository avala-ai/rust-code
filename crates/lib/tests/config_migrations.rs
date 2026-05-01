//! Integration tests for the settings migrations framework.
//!
//! Each test stages a JSON fixture in a tempdir, drives
//! `load_and_migrate`, and asserts both the in-memory result and the
//! on-disk side effects (rewrite, atomic temp file gone, backup
//! rotation, rollback on failure).
//!
//! Fixtures live in `tests/fixtures/config_migrations/`. Goldens are
//! checked verbatim against `serde_json::to_value` of the parsed file
//! so the tests are stable across whitespace differences in pretty
//! printing.

use std::fs;
use std::path::{Path, PathBuf};

use agent_code_lib::config::migrations::{
    backup_path, load_and_migrate, load_and_migrate_with, registered_migrations, run_migrations,
    Migration, CURRENT_SCHEMA_VERSION, MAX_BACKUPS, SCHEMA_VERSION_KEY,
};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tempfile::TempDir;

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("config_migrations")
        .join(name)
}

fn read_fixture(name: &str) -> String {
    fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("reading fixture {name}: {e}"))
}

fn read_fixture_value(name: &str) -> Value {
    serde_json::from_str(&read_fixture(name))
        .unwrap_or_else(|e| panic!("parsing fixture {name} as JSON: {e}"))
}

fn stage_settings(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("settings.json");
    fs::write(&path, contents).expect("staging settings.json");
    path
}

// ---- Golden migrations ----

#[test]
fn v0_file_without_schema_version_migrates_to_current_and_rewrites() {
    let tmp = TempDir::new().unwrap();
    let path = stage_settings(tmp.path(), &read_fixture("v0_input.json"));

    let (value, outcome) = load_and_migrate(&path).expect("migrate succeeds");

    assert_eq!(outcome.from_version, 0);
    assert_eq!(outcome.to_version, CURRENT_SCHEMA_VERSION);
    assert!(outcome.rewrote, "v0 file should be rewritten");
    assert_eq!(
        outcome.backup_path.as_deref(),
        Some(backup_path(&path, 1).as_path())
    );

    // In-memory matches golden.
    let expected = read_fixture_value("v0_to_current_expected.json");
    assert_eq!(value, expected);

    // On-disk file matches golden after parsing back (whitespace
    // is normalized via the JSON value comparison).
    let on_disk: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk, expected);

    // Backup contains the original pre-migration bytes.
    let bak = fs::read_to_string(backup_path(&path, 1)).unwrap();
    assert_eq!(bak.trim(), read_fixture("v0_input.json").trim());

    // Atomic temp file must not survive a successful migration.
    assert!(
        !path.with_file_name("settings.json.tmp").exists(),
        "temp file was not cleaned up by atomic rename"
    );
}

#[test]
fn v1_file_migrates_to_current() {
    let tmp = TempDir::new().unwrap();
    let path = stage_settings(tmp.path(), &read_fixture("v1_input.json"));

    let (value, outcome) = load_and_migrate(&path).expect("migrate succeeds");

    assert_eq!(outcome.from_version, 1);
    assert_eq!(outcome.to_version, CURRENT_SCHEMA_VERSION);
    assert!(outcome.rewrote);

    let expected = read_fixture_value("v1_to_current_expected.json");
    assert_eq!(value, expected);
}

// ---- No-op fast path ----

#[test]
fn current_version_file_is_not_rewritten_and_no_backup_is_created() {
    let tmp = TempDir::new().unwrap();
    let original = read_fixture("current_input.json");
    let path = stage_settings(tmp.path(), &original);
    let mtime_before = fs::metadata(&path).unwrap().modified().unwrap();

    // Sleep-free: instead of timing-based assertions, check that no
    // backups appear and that the bytes on disk are byte-identical
    // to what we wrote (rewrite would re-pretty-print).
    let (value, outcome) = load_and_migrate(&path).expect("no-op load succeeds");

    assert_eq!(outcome.from_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(outcome.to_version, CURRENT_SCHEMA_VERSION);
    assert!(!outcome.rewrote, "current-version file must not be rewritten");
    assert!(outcome.backup_path.is_none());

    // No backup file should appear.
    for n in 1..=MAX_BACKUPS {
        assert!(
            !backup_path(&path, n).exists(),
            "unexpected backup at slot {n}"
        );
    }

    // File bytes are exactly what we wrote.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, original);

    // The mtime should also be untouched.
    let mtime_after = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(mtime_before, mtime_after);

    assert_eq!(value[SCHEMA_VERSION_KEY], json!(CURRENT_SCHEMA_VERSION));
}

// ---- Backup rotation ----

#[test]
fn four_loads_keep_only_three_backups() {
    // Each "load" simulates a separate run that finds the file in a
    // pre-current state and migrates it. We intentionally migrate the
    // same file four times, so we can observe rotation behavior.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.json");

    for i in 0..4 {
        // Force a v0 file each iteration so a rewrite + backup happens.
        // Vary one field so backups have distinguishable contents.
        let body = format!(
            "{{\n  \"api\": {{ \"model\": \"m-{i}\", \"token\": \"t-{i}\" }}\n}}\n"
        );
        fs::write(&path, &body).unwrap();
        let (_value, outcome) = load_and_migrate(&path).expect("migrate succeeds");
        assert!(outcome.rewrote);
    }

    // Exactly three backup slots survive.
    for n in 1..=MAX_BACKUPS {
        assert!(
            backup_path(&path, n).exists(),
            "expected backup at slot {n}"
        );
    }
    let beyond = backup_path(&path, MAX_BACKUPS + 1);
    assert!(
        !beyond.exists(),
        "rotation should have dropped slot {} ({})",
        MAX_BACKUPS + 1,
        beyond.display()
    );

    // Slot 1 is freshest (the most recent pre-migration body, model "m-3").
    let bak1 = fs::read_to_string(backup_path(&path, 1)).unwrap();
    assert!(bak1.contains("\"m-3\""), "bak.1 should hold the latest pre-migration body, got {bak1:?}");

    // Slot 3 is oldest survivor (model "m-1"). The original "m-0"
    // archive was rotated out and dropped.
    let bak3 = fs::read_to_string(backup_path(&path, 3)).unwrap();
    assert!(bak3.contains("\"m-1\""), "bak.3 should hold the oldest survivor, got {bak3:?}");
}

#[test]
fn backup_paths_sit_next_to_settings_file() {
    let tmp = TempDir::new().unwrap();
    let path = stage_settings(tmp.path(), &read_fixture("v0_input.json"));
    let _ = load_and_migrate(&path).unwrap();

    let bak1 = backup_path(&path, 1);
    assert_eq!(bak1.parent(), path.parent());
    assert_eq!(bak1.file_name().unwrap(), "settings.json.bak.1");
}

// ---- Failure rollback ----

/// A stub migration that always errors. Used to drive the failure path
/// of `load_and_migrate` without needing a real schema bump.
struct AlwaysFails;

impl Migration for AlwaysFails {
    fn from_version(&self) -> u32 {
        0
    }
    fn to_version(&self) -> u32 {
        1
    }
    fn description(&self) -> &'static str {
        "test-only failing migration"
    }
    fn migrate(&self, _value: &mut Value) -> Result<()> {
        Err(anyhow!("synthetic failure"))
    }
}

#[test]
fn migration_failure_does_not_corrupt_file_or_create_backup() {
    let tmp = TempDir::new().unwrap();
    let original = "{ \"api\": { \"model\": \"untouched\" } }\n";
    let path = stage_settings(tmp.path(), original);

    let migrations: Vec<Box<dyn Migration>> = vec![Box::new(AlwaysFails)];
    let err = load_and_migrate_with(&path, &migrations).unwrap_err();
    assert!(
        format!("{err:#}").contains("synthetic failure"),
        "expected migration error, got: {err:#}"
    );

    // Original bytes intact.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, original, "file must not be touched on migration failure");

    // No backups, no temp turds.
    for n in 1..=MAX_BACKUPS {
        assert!(
            !backup_path(&path, n).exists(),
            "no backup should be created when migration fails"
        );
    }
    assert!(!path.with_file_name("settings.json.tmp").exists());
}

#[test]
fn parse_failure_surfaces_error_without_touching_file() {
    let tmp = TempDir::new().unwrap();
    let bad = "{not valid json";
    let path = stage_settings(tmp.path(), bad);

    let err = load_and_migrate(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("parsing settings file"), "unexpected error: {msg}");

    // File untouched on parse failure.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, bad);
    assert!(!backup_path(&path, 1).exists());
}

// ---- Downgrade detection ----

#[test]
fn higher_schema_version_is_rejected_with_clear_error() {
    let tmp = TempDir::new().unwrap();
    let body = format!(
        "{{ \"schema_version\": {} }}",
        CURRENT_SCHEMA_VERSION + 7
    );
    let path = stage_settings(tmp.path(), &body);

    let err = load_and_migrate(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Refusing to load") && msg.contains("upgrade agent-code"),
        "expected downgrade error, got: {msg}"
    );

    // Refusal must not rewrite or back up.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, body);
    assert!(!backup_path(&path, 1).exists());
}

// ---- Pure run_migrations: in-memory only ----

#[test]
fn run_migrations_pure_v0_to_current() {
    let mut v: Value = serde_json::from_str(&read_fixture("v0_input.json")).unwrap();
    let mutated = run_migrations(&mut v, &registered_migrations()).unwrap();
    assert!(mutated);
    let expected = read_fixture_value("v0_to_current_expected.json");
    assert_eq!(v, expected);
}

#[test]
fn run_migrations_pure_no_op_on_current_value() {
    let mut v = read_fixture_value("current_input.json");
    let before = v.clone();
    let mutated = run_migrations(&mut v, &registered_migrations()).unwrap();
    assert!(!mutated);
    assert_eq!(v, before);
}
