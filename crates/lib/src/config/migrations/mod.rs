//! Settings migrations framework.
//!
//! The agent's user-level settings file (`~/.config/agent-code/settings.json`)
//! and project-level settings file (`<project>/.agent/settings.json`) are
//! versioned with a top-level `"schema_version": <u32>` field. As the
//! schema evolves, we add a [`Migration`] for each version bump. On load
//! we replay every missing migration in order, write the upgraded JSON
//! back atomically (with a rolling `.bak` chain), and only then hand the
//! `serde_json::Value` to the typed deserializer.
//!
//! This module is intentionally decoupled from the existing TOML config
//! pipeline. It works on raw `serde_json::Value` so each migration is
//! a small pure function — easy to test, easy to reason about, and
//! impossible to entangle with side effects elsewhere in the loader.
//!
//! # Adding a migration
//!
//! 1. Implement [`Migration`] for a new unit struct in its own file
//!    inside this module (e.g. `v2_to_v3.rs`).
//! 2. Append the struct to [`registered_migrations`] in registration order.
//! 3. Bump [`CURRENT_SCHEMA_VERSION`] to the new `to_version`.
//! 4. Drop a fixture pair under
//!    `crates/lib/tests/fixtures/config_migrations/` and add a test in
//!    `crates/lib/tests/config_migrations.rs`.
//!
//! Migrations are pure: they take `&mut serde_json::Value` and either
//! mutate it or return `Err`. They must never touch the filesystem,
//! spawn processes, or read environment variables — the runner relies
//! on that purity for atomic-rollback semantics.

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

mod v0_to_v1;
mod v1_to_v2;

/// The schema version this build of agent-code targets. Settings files
/// with a lower `schema_version` will be migrated up to this number on
/// load. A higher version is treated as a downgrade attempt and
/// surfaced as an explicit error rather than silently dropping fields.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Top-level JSON key that tracks the schema version of a settings file.
pub const SCHEMA_VERSION_KEY: &str = "schema_version";

/// Number of `.bak.N` files to retain alongside the live settings file.
/// On each successful migrate-and-rewrite we rotate `.bak.2` → `.bak.3`,
/// `.bak.1` → `.bak.2`, then write the previous live file as `.bak.1`.
pub const MAX_BACKUPS: usize = 3;

/// A single, ordered settings-schema upgrade step.
///
/// Implementors are unit structs in `migrations/vN_to_vM.rs` files.
/// Keep migrations *pure*: mutate the JSON value in place (or return
/// `Err`) and do nothing else. The runner will not call `migrate` if
/// the file is already at or above `to_version`.
pub trait Migration: Send + Sync {
    /// Schema version this migration starts from.
    fn from_version(&self) -> u32;

    /// Schema version this migration produces.
    fn to_version(&self) -> u32;

    /// Short human-readable description for logs and doc generation.
    fn description(&self) -> &'static str;

    /// Apply the upgrade. The runner guarantees `value` is a JSON object
    /// (i.e. `value.as_object_mut()` is `Some`) before calling.
    fn migrate(&self, value: &mut serde_json::Value) -> Result<()>;
}

/// Registry of all known migrations, in application order.
///
/// Each entry MUST satisfy `to_version == next_entry.from_version`. The
/// runner asserts this contract on first call; a violation is a
/// programming error and panics rather than silently corrupting data.
pub fn registered_migrations() -> Vec<Box<dyn Migration>> {
    vec![
        Box::new(v0_to_v1::StampVersion),
        Box::new(v1_to_v2::RenameApiTokenToApiKey),
    ]
}

/// Read `schema_version` from a JSON value, defaulting to `0` when the
/// field is missing entirely (the pre-migration baseline).
///
/// Returns an error if the key is present but not a non-negative integer
/// — that's a clear authoring bug we'd rather flag than silently coerce.
pub fn read_schema_version(value: &serde_json::Value) -> Result<u32> {
    let Some(obj) = value.as_object() else {
        return Err(anyhow!(
            "settings root must be a JSON object, got {}",
            json_type_name(value)
        ));
    };
    match obj.get(SCHEMA_VERSION_KEY) {
        None => Ok(0),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| anyhow!("{SCHEMA_VERSION_KEY} must fit in u32, got {n}")),
        Some(other) => Err(anyhow!(
            "{SCHEMA_VERSION_KEY} must be a non-negative integer, got {}",
            json_type_name(other)
        )),
    }
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Apply every migration whose `from_version` is `>=` the file's current
/// version, *in order*, until the value reaches [`CURRENT_SCHEMA_VERSION`].
///
/// Returns `true` if the value was actually mutated (caller should
/// rewrite the file) or `false` if the file was already current.
///
/// On any migration error the caller's `value` is left in whatever
/// half-migrated state the failing step produced. The runner contract
/// is "all-or-nothing at the file level": the file-system runner
/// (`load_and_migrate`) snapshots the original bytes and rolls back
/// the live file before propagating the error.
pub fn run_migrations(
    value: &mut serde_json::Value,
    migrations: &[Box<dyn Migration>],
) -> Result<bool> {
    let current = read_schema_version(value)?;
    if current > CURRENT_SCHEMA_VERSION {
        return Err(anyhow!(
            "settings file declares schema_version {current}, but this build only \
             knows up to {CURRENT_SCHEMA_VERSION}. Refusing to load to avoid \
             dropping fields the newer schema introduced — upgrade agent-code or \
             pin schema_version manually."
        ));
    }
    if current == CURRENT_SCHEMA_VERSION {
        return Ok(false);
    }
    if !value.is_object() {
        return Err(anyhow!("settings root must be a JSON object"));
    }

    assert_registry_is_continuous(migrations);

    let mut version = current;
    for m in migrations {
        if m.from_version() < version {
            // Already past this step.
            continue;
        }
        if m.from_version() != version {
            return Err(anyhow!(
                "no migration registered from schema_version {version}; \
                 next available step starts at {}",
                m.from_version()
            ));
        }
        m.migrate(value).with_context(|| {
            format!("migration {} → {} failed", m.from_version(), m.to_version())
        })?;
        // Stamp the version *after* the migration succeeds so a partial
        // failure is observable: the file still reflects the source
        // version and the runner can re-attempt next load.
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                SCHEMA_VERSION_KEY.to_string(),
                serde_json::Value::from(m.to_version()),
            );
        }
        version = m.to_version();
    }

    Ok(true)
}

/// Panic if the supplied migrations don't form a continuous chain
/// (each step's `to_version` equals the next step's `from_version`).
/// Triggered by misconfigured registries, never by user data.
///
/// The "ends at CURRENT_SCHEMA_VERSION" invariant is enforced
/// separately, via a unit test on [`registered_migrations`], so that
/// callers passing a hand-crafted registry (e.g. tests of the runner
/// itself) aren't forced to advertise the canonical end-state.
fn assert_registry_is_continuous(migrations: &[Box<dyn Migration>]) {
    for window in migrations.windows(2) {
        assert_eq!(
            window[0].to_version(),
            window[1].from_version(),
            "migration registry is non-continuous: {} ends at v{} but next starts at v{}",
            window[0].description(),
            window[0].to_version(),
            window[1].from_version()
        );
    }
}

/// Outcome of a [`load_and_migrate`] call. Useful when the caller
/// wants to log "wrote 1 backup, jumped from v0 to v2".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    /// Schema version of the file before migration.
    pub from_version: u32,
    /// Schema version after migration. Equals `from_version` if no
    /// migrations needed to run.
    pub to_version: u32,
    /// True iff the file on disk was rewritten (and a backup rotated).
    pub rewrote: bool,
    /// Path to the backup that was just created, if any.
    pub backup_path: Option<PathBuf>,
}

/// Load a settings JSON file, apply pending migrations, write the
/// upgraded file back atomically, and return the resulting JSON value.
///
/// File-level guarantees:
///
/// - **Atomic write** — the upgraded JSON is written to
///   `<path>.tmp`, fsynced, then `rename`'d over the live path. Crashes
///   between steps leave either the original file untouched or the new
///   file in place — never a half-written body at `<path>`.
/// - **Backup rotation** — before the rename, the old live file is
///   moved to `<path>.bak.1`, with the previous `.bak.N` chain shifted
///   one slot older. The oldest slot beyond [`MAX_BACKUPS`] is dropped.
/// - **Migration rollback** — if any migration step returns `Err`, the
///   on-disk file is left untouched and no backup is rotated.
/// - **No-op fast path** — if the file is already at
///   [`CURRENT_SCHEMA_VERSION`] the function returns the parsed JSON
///   without touching the filesystem at all (no rewrite, no backup).
pub fn load_and_migrate(path: &Path) -> Result<(serde_json::Value, MigrationOutcome)> {
    load_and_migrate_with(path, &registered_migrations())
}

/// Same as [`load_and_migrate`] but takes an explicit migration set.
/// Exposed for tests that exercise hand-crafted registries without
/// touching the live registry.
pub fn load_and_migrate_with(
    path: &Path,
    migrations: &[Box<dyn Migration>],
) -> Result<(serde_json::Value, MigrationOutcome)> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading settings file {}", path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing settings file {} as JSON", path.display()))?;

    let from_version = read_schema_version(&value)?;
    let mutated = run_migrations(&mut value, migrations)?;
    let to_version = read_schema_version(&value)?;

    let outcome = if !mutated {
        MigrationOutcome {
            from_version,
            to_version,
            rewrote: false,
            backup_path: None,
        }
    } else {
        let backup_path = rotate_backups_and_archive(path, &raw)?;
        atomic_write_json(path, &value)?;
        MigrationOutcome {
            from_version,
            to_version,
            rewrote: true,
            backup_path: Some(backup_path),
        }
    };

    Ok((value, outcome))
}

/// Rotate `.bak.1` → `.bak.2`, etc., dropping anything past
/// [`MAX_BACKUPS`], then write the original file's bytes to `.bak.1`.
/// Returns the path to the freshly-written `.bak.1`.
fn rotate_backups_and_archive(path: &Path, original_contents: &str) -> Result<PathBuf> {
    // Drop the oldest slot if it exists. Ignore "not found" — first run
    // won't have any backups yet.
    let oldest = backup_path(path, MAX_BACKUPS);
    if oldest.exists() {
        fs::remove_file(&oldest)
            .with_context(|| format!("removing oldest backup {}", oldest.display()))?;
    }

    // Shift: .bak.{N-1} → .bak.{N}, walking from the oldest down.
    for n in (1..MAX_BACKUPS).rev() {
        let src = backup_path(path, n);
        let dst = backup_path(path, n + 1);
        if src.exists() {
            fs::rename(&src, &dst).with_context(|| {
                format!("rotating backup {} → {}", src.display(), dst.display())
            })?;
        }
    }

    // Write the pre-migration contents into .bak.1.
    let bak1 = backup_path(path, 1);
    fs::write(&bak1, original_contents)
        .with_context(|| format!("writing backup {}", bak1.display()))?;
    Ok(bak1)
}

/// Compute the path of the Nth backup slot for a settings file.
/// `n=1` is the freshest; `n=MAX_BACKUPS` is the oldest retained.
pub fn backup_path(path: &Path, n: usize) -> PathBuf {
    // Append `.bak.<n>` to the existing filename so the parent
    // directory keeps the original `settings.json` immediately
    // recognizable. PathBuf::with_extension would clobber `.json`, so
    // build the new file name by hand.
    let file_name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    let mut new_name = file_name;
    new_name.push(format!(".bak.{n}"));
    let mut out = path.to_path_buf();
    out.set_file_name(new_name);
    out
}

/// Serialize `value` as pretty JSON to `<path>.tmp`, fsync it, then
/// rename over `path`. Any error before the rename leaves the original
/// `path` untouched.
fn atomic_write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let mut tmp = path.to_path_buf();
    let mut tmp_name = tmp
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    tmp_name.push(".tmp");
    tmp.set_file_name(tmp_name);

    let bytes = serde_json::to_vec_pretty(value).context("serializing migrated settings")?;

    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("opening temp file {}", tmp.display()))?;
        f.write_all(&bytes)
            .with_context(|| format!("writing temp file {}", tmp.display()))?;
        f.write_all(b"\n").ok();
        f.sync_all()
            .with_context(|| format!("fsyncing temp file {}", tmp.display()))?;
    }

    fs::rename(&tmp, path).with_context(|| {
        format!(
            "renaming temp file {} → {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_schema_version_defaults_to_zero_when_absent() {
        let v = json!({"api": {"model": "x"}});
        assert_eq!(read_schema_version(&v).unwrap(), 0);
    }

    #[test]
    fn read_schema_version_reads_explicit_value() {
        let v = json!({"schema_version": 3, "api": {}});
        assert_eq!(read_schema_version(&v).unwrap(), 3);
    }

    #[test]
    fn read_schema_version_rejects_non_integer() {
        let v = json!({"schema_version": "two"});
        assert!(read_schema_version(&v).is_err());
    }

    #[test]
    fn read_schema_version_rejects_negative() {
        let v = json!({"schema_version": -1});
        assert!(read_schema_version(&v).is_err());
    }

    #[test]
    fn read_schema_version_rejects_non_object_root() {
        let v = json!([1, 2, 3]);
        assert!(read_schema_version(&v).is_err());
    }

    #[test]
    fn registered_chain_is_continuous_and_ends_at_current() {
        let migrations = registered_migrations();
        assert!(!migrations.is_empty(), "registry must not be empty while CURRENT_SCHEMA_VERSION > 0");
        assert_registry_is_continuous(&migrations);
        // Lowest entry must start at 0 so a brand-new file (no
        // schema_version field) can be picked up.
        assert_eq!(migrations.first().unwrap().from_version(), 0);
        // Last entry must take the file all the way to the current
        // build's target version — anything else is a packaging bug.
        assert_eq!(
            migrations.last().unwrap().to_version(),
            CURRENT_SCHEMA_VERSION,
            "last registered migration must end at CURRENT_SCHEMA_VERSION"
        );
    }

    #[test]
    fn run_migrations_no_op_when_already_current() {
        let mut v = json!({"schema_version": CURRENT_SCHEMA_VERSION});
        let mutated = run_migrations(&mut v, &registered_migrations()).unwrap();
        assert!(!mutated);
        assert_eq!(read_schema_version(&v).unwrap(), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn run_migrations_upgrades_v0_to_current() {
        let mut v = json!({"api": {"model": "test"}});
        let mutated = run_migrations(&mut v, &registered_migrations()).unwrap();
        assert!(mutated);
        assert_eq!(read_schema_version(&v).unwrap(), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn run_migrations_rejects_future_version() {
        let mut v = json!({"schema_version": CURRENT_SCHEMA_VERSION + 5});
        let err = run_migrations(&mut v, &registered_migrations()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Refusing to load"), "got: {msg}");
    }

    #[test]
    fn backup_path_appends_dot_bak_n_to_full_filename() {
        let p = Path::new("/tmp/settings.json");
        assert_eq!(backup_path(p, 1), Path::new("/tmp/settings.json.bak.1"));
        assert_eq!(backup_path(p, 3), Path::new("/tmp/settings.json.bak.3"));
    }
}
