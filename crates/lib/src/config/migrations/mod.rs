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

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Default permission mode applied when we have no existing file to
/// copy mode bits from. Settings files routinely contain API keys, so
/// the safer default is owner-read/write only — matching what tools
/// like ssh use for key files. POSIX-only; ignored on Windows.
#[cfg(unix)]
const DEFAULT_SETTINGS_MODE: u32 = 0o600;

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
    // `from_version` reads as "the version this migration starts from",
    // not as a fallible constructor — the wrong_self_convention lint
    // misfires on accessor pairs like `from_version`/`to_version`.
    #[allow(clippy::wrong_self_convention)]
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
/// - **Backup rotation** — only after the temp file is renamed into
///   place do we rotate the `.bak.N` chain and archive the old live
///   file's bytes (snapshotted before the rename) into `<path>.bak.1`.
///   Rotation never runs on a path where the new write failed.
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
        // Order matters here: any failure between "open temp file" and
        // "rename" must leave the live file *and* the backup chain
        // exactly as we found them. So: write the new bytes to a temp
        // file, fsync, rename over the live path, and only then archive
        // the snapshot we took into `.bak.1` with rotation. If the
        // archive step fails the live file is already up-to-date — we
        // surface the error but the caller's data is not lost.
        //
        // Capture the live file's permission mode *before* the rewrite
        // so we can apply it to both the rewritten live file and the
        // archived `.bak.1` — preserves a 0600 secrets file's mode
        // instead of letting umask widen it to 0644.
        let mode = PreservedMode::capture(path);
        atomic_write_bytes(path, &serialize_pretty_json(&value)?, true, mode)?;
        let backup_path = rotate_backups_and_archive(path, &raw, mode)?;
        MigrationOutcome {
            from_version,
            to_version,
            rewrote: true,
            backup_path: Some(backup_path),
        }
    };

    Ok((value, outcome))
}

/// Serialize a JSON value as pretty-printed bytes for an atomic write.
/// Tiny helper kept separate from [`atomic_write_bytes`] so the byte
/// payload and the captured permission mode can be passed alongside
/// each other.
fn serialize_pretty_json(value: &serde_json::Value) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).context("serializing migrated settings")
}

/// Rotate `.bak.1` → `.bak.2`, etc., dropping anything past
/// [`MAX_BACKUPS`], then write the supplied pre-migration bytes to
/// `.bak.1`. Returns the path to the freshly-written `.bak.1`.
///
/// Callers must invoke this **after** the new live file is safely in
/// place. The rotation is not itself atomic across slots; a crash
/// mid-rotation would leave the chain re-numbered but the live file
/// already correctly migrated, which is the failure mode we prefer.
///
/// `mode` is the permission mode to apply to the freshly-written
/// `.bak.1` on Unix — the same mode bits we capture from the live
/// file so backups don't widen access on a 0600 secrets file.
fn rotate_backups_and_archive(
    path: &Path,
    original_contents: &str,
    mode: PreservedMode,
) -> Result<PathBuf> {
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

    // Write the pre-migration contents into .bak.1, going through the
    // same atomic-write helper as the live file so the backup also
    // gets unguessable temp-file naming, mode preservation, and a
    // robust rename across platforms.
    let bak1 = backup_path(path, 1);
    atomic_write_bytes(&bak1, original_contents.as_bytes(), false, mode)
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

/// Permission mode captured from a pre-existing live file (or
/// defaulted to a secrets-safe value when no file exists yet). On
/// Windows this only tracks the read-only flag; POSIX mode bits have
/// no Windows equivalent worth round-tripping.
#[derive(Debug, Clone, Copy)]
struct PreservedMode {
    /// POSIX mode (e.g. `0o600`). Only meaningful on Unix; on Windows
    /// this is ignored at apply time.
    #[cfg_attr(not(unix), allow(dead_code))]
    unix_mode: u32,
    /// Windows read-only flag, captured from the existing file's
    /// permissions if any. POSIX-mode is not mappable on Windows, so
    /// we preserve the closest-fit attribute instead.
    #[cfg_attr(not(windows), allow(dead_code))]
    windows_readonly: bool,
}

impl PreservedMode {
    /// Capture mode from an existing file, falling back to a
    /// secrets-safe default when the file doesn't exist yet.
    fn capture(path: &Path) -> Self {
        match fs::metadata(path) {
            Ok(meta) => {
                let perms = meta.permissions();
                #[cfg(unix)]
                let unix_mode = {
                    use std::os::unix::fs::PermissionsExt;
                    perms.mode()
                };
                #[cfg(not(unix))]
                let unix_mode = 0;
                Self {
                    unix_mode,
                    windows_readonly: perms.readonly(),
                }
            }
            Err(_) => Self::default_for_secrets(),
        }
    }

    /// Default for a file that does not exist yet. Settings files
    /// routinely contain API keys, so the safer default is
    /// owner-read/write only on Unix and not-readonly on Windows.
    fn default_for_secrets() -> Self {
        Self {
            #[cfg(unix)]
            unix_mode: DEFAULT_SETTINGS_MODE,
            #[cfg(not(unix))]
            unix_mode: 0,
            windows_readonly: false,
        }
    }

    /// Apply the captured mode to `target`. POSIX mode bits are
    /// applied verbatim on Unix; on Windows we only preserve the
    /// read-only flag (POSIX modes have no faithful Windows
    /// equivalent).
    fn apply(self, target: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(self.unix_mode);
            fs::set_permissions(target, perms).with_context(|| {
                format!(
                    "setting permissions {:o} on {}",
                    self.unix_mode,
                    target.display()
                )
            })?;
        }
        #[cfg(windows)]
        {
            let mut perms = fs::metadata(target)
                .with_context(|| format!("reading permissions for {}", target.display()))?
                .permissions();
            if perms.readonly() != self.windows_readonly {
                perms.set_readonly(self.windows_readonly);
                fs::set_permissions(target, perms)
                    .with_context(|| format!("setting permissions on {}", target.display()))?;
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = target;
        }
        Ok(())
    }
}

/// Atomically replace `path` with `bytes`.
///
/// Implementation:
///
/// - Writes to a randomly-named temp file in the same directory as
///   `path` (via `tempfile::NamedTempFile::new_in`), so a pre-planted
///   symlink at a deterministic `<path>.tmp` cannot get truncated.
/// - Applies the supplied permission mode (e.g. `0o600`) to the temp
///   file before writing — preserves an existing 0600 secrets file's
///   restrictive mode instead of letting umask widen it to 0644.
/// - Fsyncs the temp file before persisting.
/// - Uses `NamedTempFile::persist`, which on Windows uses
///   `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` so renaming over
///   an existing destination works.
/// - On POSIX, fsyncs the parent directory after the rename so the
///   directory entry is durable across power loss. Skipped on Windows
///   (no equivalent — `MoveFileExW` already handles durability).
///
/// `append_newline` controls whether a trailing newline is appended to
/// the written body (matches existing JSON-write behavior; useful for
/// TOML callers that already produce a trailing newline themselves).
fn atomic_write_bytes(
    path: &Path,
    bytes: &[u8],
    append_newline: bool,
    mode: PreservedMode,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Unguessable temp-file path: tempfile generates a random suffix
    // and refuses pre-existing files, defeating planted-symlink
    // truncation on the deterministic `<path>.tmp` slot.
    let mut named = tempfile::NamedTempFile::new_in(&parent)
        .with_context(|| format!("creating temp file in {}", parent.display()))?;

    // Apply the captured mode *before* writing secrets, so the file
    // is never even briefly readable by other users.
    mode.apply(named.path())?;

    {
        let tmp_path_display = named.path().display().to_string();
        let f = named.as_file_mut();
        f.write_all(bytes)
            .with_context(|| format!("writing temp file {tmp_path_display}"))?;
        if append_newline {
            f.write_all(b"\n").ok();
        }
        f.sync_all()
            .with_context(|| format!("fsyncing temp file {tmp_path_display}"))?;
    }

    // Persist atomically over the live path. On Windows this uses
    // ReplaceFile/MoveFileExW under the hood, so renaming over an
    // existing target works (unlike `std::fs::rename` on Windows).
    named.persist(path).map_err(|e| {
        anyhow!(
            "renaming temp file {} → {}: {}",
            e.file.path().display(),
            path.display(),
            e.error
        )
    })?;

    // POSIX: fsync the parent directory so the rename's directory
    // entry survives a crash. Windows has no equivalent and the
    // ReplaceFile path already provides durability.
    #[cfg(unix)]
    {
        if let Ok(dir) = fs::File::open(&parent) {
            // sync_all on a directory may fail on some unusual
            // filesystems; downgrade to a best-effort step rather
            // than failing the whole migration.
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

/// Outcome of running migrations against a TOML settings file. Same
/// shape as [`MigrationOutcome`] so callers can log uniformly.
///
/// # Format limitations
///
/// Migrations are expressed as pure mutations of `serde_json::Value`,
/// so the TOML runner converts via `toml::Value` ↔ `serde_json::Value`
/// before and after the migration chain. That conversion is **lossy
/// for TOML datetime values**: any `datetime`/`local-date`/`local-time`
/// in the file is serialized to its RFC 3339 string form on the way
/// in and remains a string on the way out. The current settings
/// schema has no datetime fields, so this is documented but not
/// triggered by any shipping migration. If a future migration needs
/// to round-trip a datetime, switch that field to a string in the
/// schema first.
///
/// Comments are also lost on rewrite (the `toml` crate's default
/// serializer emits canonical-form output without preserving the
/// original token stream). This matches the existing `Config` loader,
/// which already strips comments on every read-merge-write cycle.
pub fn load_and_migrate_toml(path: &Path) -> Result<(toml::Value, MigrationOutcome)> {
    load_and_migrate_toml_with(path, &registered_migrations())
}

/// Same as [`load_and_migrate_toml`] but with an explicit migration
/// set, mirroring [`load_and_migrate_with`].
pub fn load_and_migrate_toml_with(
    path: &Path,
    migrations: &[Box<dyn Migration>],
) -> Result<(toml::Value, MigrationOutcome)> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading settings file {}", path.display()))?;
    let toml_value: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("parsing settings file {} as TOML", path.display()))?;

    // Convert to JSON for the migration chain. Pure migrations only
    // see `serde_json::Value`, so the runner is format-agnostic above
    // this boundary.
    let mut json_value: serde_json::Value = toml_to_json(&toml_value)?;

    let from_version = read_schema_version(&json_value)?;
    let mutated = run_migrations(&mut json_value, migrations)?;
    let to_version = read_schema_version(&json_value)?;

    let migrated_toml = json_to_toml(&json_value)?;

    let outcome = if !mutated {
        MigrationOutcome {
            from_version,
            to_version,
            rewrote: false,
            backup_path: None,
        }
    } else {
        let bytes = toml::to_string_pretty(&migrated_toml)
            .context("serializing migrated settings as TOML")?
            .into_bytes();
        // toml::to_string_pretty already emits a trailing newline.
        let mode = PreservedMode::capture(path);
        atomic_write_bytes(path, &bytes, false, mode)?;
        let backup_path = rotate_backups_and_archive(path, &raw, mode)?;
        MigrationOutcome {
            from_version,
            to_version,
            rewrote: true,
            backup_path: Some(backup_path),
        }
    };

    Ok((migrated_toml, outcome))
}

/// Convert a `toml::Value` to a `serde_json::Value`, used as the
/// crossing between the TOML on-disk format and the JSON-shaped
/// migration runner. Lossy for TOML datetimes (see
/// [`load_and_migrate_toml`] docs).
fn toml_to_json(value: &toml::Value) -> Result<serde_json::Value> {
    match value {
        toml::Value::String(s) => Ok(serde_json::Value::String(s.clone())),
        toml::Value::Integer(i) => Ok(serde_json::Value::from(*i)),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| anyhow!("non-finite TOML float {f} cannot be represented as JSON")),
        toml::Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        // TOML datetimes have no JSON counterpart; serialize to the
        // RFC 3339 string form so the migration chain can still read
        // and pass through the value. No current migration touches
        // such a field.
        toml::Value::Datetime(dt) => Ok(serde_json::Value::String(dt.to_string())),
        toml::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(toml_to_json(item)?);
            }
            Ok(serde_json::Value::Array(out))
        }
        toml::Value::Table(table) => {
            let mut out = serde_json::Map::with_capacity(table.len());
            for (k, v) in table {
                out.insert(k.clone(), toml_to_json(v)?);
            }
            Ok(serde_json::Value::Object(out))
        }
    }
}

/// Convert a `serde_json::Value` produced by the migration chain back
/// to a `toml::Value` for on-disk writing. The migration chain only
/// produces objects/arrays/strings/numbers/bools, so the conversion
/// is total for all values our migrations emit.
fn json_to_toml(value: &serde_json::Value) -> Result<toml::Value> {
    match value {
        serde_json::Value::Null => Err(anyhow!(
            "cannot represent JSON null in TOML; migrations must remove the key instead of \
             writing a null value"
        )),
        serde_json::Value::Bool(b) => Ok(toml::Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(toml::Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(toml::Value::Float(f))
            } else {
                Err(anyhow!("JSON number {n} does not fit in TOML i64 or f64"))
            }
        }
        serde_json::Value::String(s) => Ok(toml::Value::String(s.clone())),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_toml(item)?);
            }
            Ok(toml::Value::Array(out))
        }
        serde_json::Value::Object(obj) => {
            let mut out = toml::value::Table::new();
            for (k, v) in obj {
                // Skip keys that ended up as JSON null after a
                // migration — TOML cannot represent them.
                if v.is_null() {
                    continue;
                }
                out.insert(k.clone(), json_to_toml(v)?);
            }
            Ok(toml::Value::Table(out))
        }
    }
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
        assert!(
            !migrations.is_empty(),
            "registry must not be empty while CURRENT_SCHEMA_VERSION > 0"
        );
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
