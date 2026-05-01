//! Named configuration profiles.
//!
//! A profile is a snapshot of [`Config`] persisted to disk under
//! `<config_dir>/agent-code/profiles/<name>.toml`. Profiles let users
//! switch between named setups (e.g. "work", "personal", "yolo")
//! without editing the main config.toml or exporting env vars.
//!
//! Profiles are full config snapshots — loading one replaces the
//! runtime config wholesale rather than merging. That's deliberate:
//! merging two user-maintained configs is where most "why is this
//! setting ignored?" bugs come from.

use std::path::PathBuf;

use crate::config::Config;
use crate::config::migrations;

/// Resolve the directory that holds profile TOML files.
///
/// Returns `None` when no config directory can be determined (e.g.
/// HOME is unset on Unix) — callers should surface a friendly error.
fn profiles_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("profiles"))
}

/// Path for a specific profile name.
fn profile_path(name: &str) -> Option<PathBuf> {
    profiles_dir().map(|d| d.join(format!("{name}.toml")))
}

/// Validate a profile name. Rejects anything that could escape the
/// profiles directory or break shell completion.
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("profile name is empty".to_string());
    }
    if name.len() > 64 {
        return Err("profile name exceeds 64 characters".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "profile name '{name}' contains disallowed characters \
             (allowed: letters, digits, '-', '_')"
        ));
    }
    Ok(())
}

/// Save the given config as a named profile. Overwrites any existing
/// profile with the same name.
pub fn save_profile(name: &str, config: &Config) -> Result<PathBuf, String> {
    validate_name(name)?;
    let path = profile_path(name).ok_or("could not resolve profiles directory")?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create profiles dir: {e}"))?;
    }

    let toml = toml::to_string_pretty(config)
        .map_err(|e| format!("failed to serialize config to TOML: {e}"))?;
    std::fs::write(&path, toml).map_err(|e| format!("failed to write profile: {e}"))?;
    Ok(path)
}

/// Load the named profile and return the fully-typed config.
///
/// Routes the parsed TOML through the same schema-migration runner
/// that `Config::load` uses, so old profile snapshots (e.g. ones with
/// the legacy `api.token` field) get upgraded before typed
/// deserialization. When the migration chain rewrites the value the
/// updated bytes are persisted back to the profile file with the
/// same atomic-write/backup guarantees as the main settings file.
///
/// Does NOT apply environment-variable overrides (those still take
/// effect on the running process). Callers that want env precedence
/// should call `Config::load()` after loading a profile.
pub fn load_profile(name: &str) -> Result<Config, String> {
    validate_name(name)?;
    let path = profile_path(name).ok_or("could not resolve profiles directory")?;
    if !path.exists() {
        return Err(format!("profile '{name}' not found at {}", path.display()));
    }
    // Drive the on-disk migration runner so old profile snapshots get
    // both upgraded in memory and rewritten on disk (with backup
    // rotation) the same way `Config::load` handles `settings.toml`.
    let (migrated, _outcome) = migrations::load_and_migrate_toml(&path)
        .map_err(|e| format!("failed to migrate profile: {e:#}"))?;
    let config: Config = migrated
        .try_into()
        .map_err(|e| format!("failed to parse profile TOML: {e}"))?;
    Ok(config)
}

/// Delete a named profile. Returns `Ok(false)` if it didn't exist,
/// `Ok(true)` if it was removed.
pub fn delete_profile(name: &str) -> Result<bool, String> {
    validate_name(name)?;
    let path = profile_path(name).ok_or("could not resolve profiles directory")?;
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| format!("failed to delete profile: {e}"))?;
    Ok(true)
}

/// Summary of a profile suitable for listing.
#[derive(Debug)]
pub struct ProfileSummary {
    pub name: String,
    pub path: PathBuf,
    pub model: String,
}

/// List saved profiles, sorted alphabetically. Returns an empty vec
/// when the profiles dir doesn't exist yet.
pub fn list_profiles() -> Vec<ProfileSummary> {
    let Some(dir) = profiles_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut out: Vec<ProfileSummary> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "toml") {
                return None;
            }
            let name = path.file_stem()?.to_string_lossy().to_string();
            // Best-effort model read; if the profile is malformed we
            // still list it so the user can delete it.
            let model = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str::<Config>(&s).ok())
                .map(|c| c.api.model)
                .unwrap_or_else(|| "(unparseable)".to_string());
            Some(ProfileSummary { name, path, model })
        })
        .collect();

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_safe_names() {
        assert!(validate_name("work").is_ok());
        assert!(validate_name("work_2").is_ok());
        assert!(validate_name("ci-mode").is_ok());
        assert!(validate_name("a").is_ok());
    }

    #[test]
    fn validate_name_rejects_empty() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn validate_name_rejects_path_escape() {
        // Most important: don't let a profile name write outside profiles/.
        assert!(validate_name("../etc/passwd").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("/absolute").is_err());
        assert!(validate_name("foo/bar").is_err());
        assert!(validate_name("foo\\bar").is_err());
    }

    #[test]
    fn validate_name_rejects_special_chars() {
        assert!(validate_name("foo bar").is_err());
        assert!(validate_name("foo.bar").is_err());
        assert!(validate_name("foo;bar").is_err());
        assert!(validate_name("foo*").is_err());
    }

    #[test]
    fn validate_name_rejects_oversize() {
        let long = "a".repeat(65);
        assert!(validate_name(&long).is_err());
    }

    /// Drive the v0 → current migration through the public profile
    /// loader: a snapshot with the legacy `api.token` field must come
    /// out with `api.api_key` populated and the on-disk file must be
    /// rewritten with `schema_version` stamped (and a `.bak.1`
    /// archived).
    ///
    /// `dirs::config_dir()` resolves from `XDG_CONFIG_HOME` on Linux
    /// and `HOME` on macOS, so we point one of those at a tempdir and
    /// stage the profile inside it. Tests in this module run
    /// single-threaded by default; the mutex below makes it explicit.
    #[test]
    fn load_profile_migrates_legacy_token_field_and_rewrites_on_disk() {
        use crate::config::migrations::{CURRENT_SCHEMA_VERSION, backup_path};
        use std::fs;

        // Serialize against any other test in this binary that
        // mutates process-global env state.
        static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());

        let tmp = tempfile::tempdir().unwrap();
        let config_root = tmp.path().to_path_buf();

        // Stash both XDG_CONFIG_HOME (Linux) and HOME (macOS) so the
        // test works on either platform's `dirs::config_dir`.
        let saved_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let saved_home = std::env::var_os("HOME");
        // SAFETY: serialized via ENV_GUARD; other tests in this
        // module take the same lock before mutating env vars.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &config_root);
            std::env::set_var("HOME", &config_root);
        }

        // Stage a v0 profile snapshot with the legacy `api.token` field.
        let profile_dir = config_root.join("agent-code").join("profiles");
        fs::create_dir_all(&profile_dir).unwrap();
        let profile_file = profile_dir.join("legacy.toml");
        fs::write(
            &profile_file,
            "[api]\nmodel = \"legacy-gpt\"\ntoken = \"sk-legacy-from-profile\"\n",
        )
        .unwrap();

        let load_result = load_profile("legacy");

        // Restore env before asserting so a panic doesn't leak state.
        unsafe {
            match saved_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        let cfg = load_result.expect("profile loads after migration");
        assert_eq!(cfg.api.model, "legacy-gpt");
        assert_eq!(
            cfg.api.api_key.as_deref(),
            Some("sk-legacy-from-profile"),
            "v1 → v2 migration should have moved api.token into api.api_key"
        );

        // On-disk profile was rewritten: schema_version stamped,
        // legacy `token` removed, `api_key` written.
        let on_disk: toml::Value = toml::from_str(&fs::read_to_string(&profile_file).unwrap())
            .expect("rewritten profile parses");
        assert_eq!(
            on_disk
                .get("schema_version")
                .and_then(|v| v.as_integer())
                .map(|n| n as u32),
            Some(CURRENT_SCHEMA_VERSION),
            "profile should be rewritten with schema_version stamped"
        );
        let api = on_disk.get("api").and_then(|v| v.as_table()).unwrap();
        assert!(!api.contains_key("token"), "legacy token field removed");
        assert_eq!(
            api.get("api_key").and_then(|v| v.as_str()),
            Some("sk-legacy-from-profile")
        );

        // `.bak.1` archives the pre-migration body.
        let bak1 = fs::read_to_string(backup_path(&profile_file, 1)).unwrap();
        assert!(bak1.contains("token = \"sk-legacy-from-profile\""));
    }
}
