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
/// Does NOT apply environment-variable overrides (those still take
/// effect on the running process). Callers that want env precedence
/// should call `Config::load()` after loading a profile.
pub fn load_profile(name: &str) -> Result<Config, String> {
    validate_name(name)?;
    let path = profile_path(name).ok_or("could not resolve profiles directory")?;
    if !path.exists() {
        return Err(format!("profile '{name}' not found at {}", path.display()));
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("failed to read profile: {e}"))?;
    let config: Config =
        toml::from_str(&text).map_err(|e| format!("failed to parse profile TOML: {e}"))?;
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
}
