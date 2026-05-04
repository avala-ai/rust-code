//! Allow-list of settings that the model-callable `Config` tool may
//! read or write.
//!
//! The `Config` tool exposes a deliberately narrow surface: the model
//! can only touch keys defined here. Anything else is rejected at
//! validation time with a clear error. The list is small on purpose —
//! when in doubt, leave a setting out. New entries should be:
//!
//! 1. Already present in [`crate::config::Config`] / [`schema`].
//! 2. Clearly user-tunable (theme, default model, opt-in flags).
//! 3. Not security-sensitive (never API keys, sandbox toggles,
//!    permission rules, allow-lists, hooks, MCP server configs).
//!
//! [`schema`]: crate::config::schema

/// The kind of value a setting accepts. Drives validation in the
/// `Config` tool: a `set` request is rejected before it ever reaches
/// the on-disk TOML when the supplied value doesn't match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingKind {
    /// Boolean (`true` / `false`).
    Bool,
    /// Signed 64-bit integer.
    Int,
    /// Floating-point number.
    Float,
    /// Free-form string.
    String,
    /// String constrained to one of a small set of literal values.
    Enum(&'static [&'static str]),
}

/// Where a setting lives on disk. The `Config` tool writes to the
/// matching layer's TOML file. The user layer is the user's
/// `config_dir/agent-code/config.toml`; the project layer is the
/// nearest `.agent/settings.toml` in or above the working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// User-level settings (`~/.config/agent-code/config.toml`).
    User,
    /// Project-level settings (`.agent/settings.toml`).
    Project,
}

/// One entry in the allow-list of model-tunable settings.
///
/// The `key` field uses dotted notation that mirrors the on-disk TOML
/// (e.g. `ui.theme` for `[ui] theme = "..."`). The `Config` tool
/// resolves that path inside the matching scope's TOML document.
#[derive(Debug, Clone)]
pub struct SupportedSetting {
    /// Dotted TOML path (e.g. `"ui.theme"`).
    pub key: &'static str,
    /// Human-readable description for `list_supported`.
    pub description: &'static str,
    /// Expected value kind — drives validation on `set`.
    pub kind: SettingKind,
    /// Which on-disk file owns this setting.
    pub scope: Scope,
}

/// The full allow-list. Order is preserved for `list_supported` so
/// related settings (UI, features) cluster together rather than
/// shuffling on every recompile.
pub const SUPPORTED_SETTINGS: &[SupportedSetting] = &[
    // ---- UI ----
    SupportedSetting {
        key: "ui.theme",
        description: "Color theme for the terminal UI.",
        kind: SettingKind::Enum(&["dark", "light", "solarized", "auto"]),
        scope: Scope::User,
    },
    SupportedSetting {
        key: "ui.markdown",
        description: "Render Markdown output (bold, headings, lists) in the REPL.",
        kind: SettingKind::Bool,
        scope: Scope::User,
    },
    SupportedSetting {
        key: "ui.syntax_highlight",
        description: "Highlight code blocks in tool output.",
        kind: SettingKind::Bool,
        scope: Scope::User,
    },
    SupportedSetting {
        key: "ui.edit_mode",
        description: "Line-editor key bindings: \"emacs\" or \"vi\".",
        kind: SettingKind::Enum(&["emacs", "vi"]),
        scope: Scope::User,
    },
    // ---- Features ----
    SupportedSetting {
        key: "features.auto_theme",
        description: "Auto-detect system dark/light mode for the terminal theme.",
        kind: SettingKind::Bool,
        scope: Scope::User,
    },
    SupportedSetting {
        key: "features.commit_attribution",
        description: "Append a co-author trailer to commits the agent creates.",
        kind: SettingKind::Bool,
        scope: Scope::User,
    },
    SupportedSetting {
        key: "features.token_budget",
        description: "Track per-turn token usage and warn when approaching the budget.",
        kind: SettingKind::Bool,
        scope: Scope::User,
    },
    // ---- Project-scoped ----
    SupportedSetting {
        key: "api.model",
        description: "Default model identifier for this project.",
        kind: SettingKind::String,
        scope: Scope::Project,
    },
];

/// Look up a setting by its dotted key. Returns `None` for any key
/// not on the allow-list — the caller must reject the request.
pub fn lookup(key: &str) -> Option<&'static SupportedSetting> {
    SUPPORTED_SETTINGS.iter().find(|s| s.key == key)
}

/// Validate a JSON value against this setting's [`SettingKind`].
/// Returns the canonicalized [`toml::Value`] on success so the caller
/// can write it back to disk without re-parsing.
pub fn coerce_value(
    setting: &SupportedSetting,
    value: &serde_json::Value,
) -> Result<toml::Value, String> {
    match (&setting.kind, value) {
        (SettingKind::Bool, serde_json::Value::Bool(b)) => Ok(toml::Value::Boolean(*b)),
        (SettingKind::Bool, _) => Err(format!("setting '{}' expects a boolean", setting.key)),

        (SettingKind::Int, serde_json::Value::Number(n)) => n
            .as_i64()
            .map(toml::Value::Integer)
            .ok_or_else(|| format!("setting '{}' expects an integer", setting.key)),
        (SettingKind::Int, _) => Err(format!("setting '{}' expects an integer", setting.key)),

        (SettingKind::Float, serde_json::Value::Number(n)) => n
            .as_f64()
            .map(toml::Value::Float)
            .ok_or_else(|| format!("setting '{}' expects a float", setting.key)),
        (SettingKind::Float, _) => Err(format!("setting '{}' expects a number", setting.key)),

        (SettingKind::String, serde_json::Value::String(s)) => Ok(toml::Value::String(s.clone())),
        (SettingKind::String, _) => Err(format!("setting '{}' expects a string", setting.key)),

        (SettingKind::Enum(allowed), serde_json::Value::String(s)) => {
            if allowed.contains(&s.as_str()) {
                Ok(toml::Value::String(s.clone()))
            } else {
                Err(format!(
                    "setting '{}' must be one of {:?} (got {:?})",
                    setting.key, allowed, s
                ))
            }
        }
        (SettingKind::Enum(_), _) => Err(format!(
            "setting '{}' expects one of its enum string values",
            setting.key
        )),
    }
}

/// Section names whose contents are off-limits to the model-callable
/// `Config` tool. Compared as separator-aware lowercase top-level
/// segments — `permissionsx` does not match, `permissions.foo` does.
const BLOCKED_SECTIONS: &[&str] = &["permissions", "security", "sandbox", "hooks", "mcp_servers"];

/// Return true when `key`'s first dotted segment (lowercased) matches
/// one of the blocked section names exactly. Used both by the
/// allow-list tripwire test and by the `Config` tool's runtime
/// validator so that `set permissions.default_mode = "allow"` is
/// rejected up front, regardless of what's on the allow-list.
pub fn is_security_sensitive_section(key: &str) -> bool {
    let head = key.split('.').next().unwrap_or("").to_ascii_lowercase();
    BLOCKED_SECTIONS.iter().any(|blocked| head == *blocked)
}

/// Return true when *any* dotted segment of `key`, lowercased,
/// contains one of the API-key markers (`api_key`, `api-key`,
/// `apikey`). Catches `theme.api_key_color` as well as bare
/// `API_KEY`. Invariant for the allow-list and the runtime
/// validator alike.
pub fn any_segment_matches_api_key(key: &str) -> bool {
    key.split('.').any(|segment| {
        let lower = segment.to_ascii_lowercase();
        lower.contains("api_key") || lower.contains("api-key") || lower.contains("apikey")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allowlist_is_small_and_unique() {
        // Be conservative: better to ship a small allow-list than a
        // wide one. Keep this assertion as a tripwire.
        assert!(SUPPORTED_SETTINGS.len() <= 12);

        let mut keys: Vec<&str> = SUPPORTED_SETTINGS.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        let dedup_len = {
            let mut k = keys.clone();
            k.dedup();
            k.len()
        };
        assert_eq!(keys.len(), dedup_len, "duplicate keys in allow-list");
    }

    #[test]
    fn lookup_returns_known_keys() {
        assert!(lookup("ui.theme").is_some());
        assert!(lookup("features.commit_attribution").is_some());
        assert!(lookup("nonexistent.key").is_none());
    }

    #[test]
    fn coerce_bool_accepts_only_bool() {
        let s = lookup("ui.markdown").unwrap();
        assert_eq!(
            coerce_value(s, &json!(true)).unwrap(),
            toml::Value::Boolean(true)
        );
        assert!(coerce_value(s, &json!("true")).is_err());
        assert!(coerce_value(s, &json!(1)).is_err());
    }

    #[test]
    fn coerce_enum_rejects_unlisted_value() {
        let s = lookup("ui.theme").unwrap();
        assert!(coerce_value(s, &json!("dark")).is_ok());
        assert!(coerce_value(s, &json!("magenta")).is_err());
        assert!(coerce_value(s, &json!(42)).is_err());
    }

    #[test]
    fn coerce_string_accepts_string_only() {
        let s = lookup("api.model").unwrap();
        assert!(coerce_value(s, &json!("gpt-5.4")).is_ok());
        assert!(coerce_value(s, &json!(7)).is_err());
    }

    #[test]
    fn coerce_int_accepts_integer_numbers() {
        // Synthesise a fake int setting for the test rather than
        // adding one to the real allow-list.
        let s = SupportedSetting {
            key: "tmp.int",
            description: "",
            kind: SettingKind::Int,
            scope: Scope::User,
        };
        assert!(coerce_value(&s, &json!(42)).is_ok());
        assert!(coerce_value(&s, &json!(1.5)).is_err());
        assert!(coerce_value(&s, &json!("42")).is_err());
    }

    #[test]
    fn no_security_sensitive_keys_in_allowlist() {
        // Tripwire — any of these would be a footgun. Match by
        // separator-aware top-level segment, case-insensitively,
        // so neighbouring section names that merely share a prefix
        // (e.g. `permissionsx`) don't get spuriously flagged but
        // `permissions.foo` still does.
        for s in SUPPORTED_SETTINGS {
            assert!(
                !is_security_sensitive_section(s.key),
                "allow-list entry {:?} sits in a security-sensitive section",
                s.key
            );
            assert!(
                !any_segment_matches_api_key(s.key),
                "allow-list entry {:?} looks like an API key",
                s.key
            );
        }
    }

    #[test]
    fn tripwire_separator_aware_prefix_check() {
        // The tripwire matches whole section names, not raw substrings.
        assert!(is_security_sensitive_section("permissions.default_mode"));
        assert!(is_security_sensitive_section("PERMISSIONS.foo")); // case-insensitive
        assert!(is_security_sensitive_section("permissions")); // exact section name
        assert!(!is_security_sensitive_section("permissionsx.foo")); // not a real prefix
        assert!(!is_security_sensitive_section("ui.permissions_label")); // not a top-level section
        assert!(!is_security_sensitive_section("theme")); // unrelated
    }

    #[test]
    fn tripwire_api_key_segment_check_covers_variants() {
        // Every segment of every dotted key is examined,
        // case-insensitively, for `api_key` / `api-key` / `apikey`.
        assert!(any_segment_matches_api_key("API_KEY"));
        assert!(any_segment_matches_api_key("api-key"));
        assert!(any_segment_matches_api_key("apikey"));
        assert!(any_segment_matches_api_key("theme.api_key_color")); // substring within segment
        assert!(any_segment_matches_api_key("ui.APIKEY_color"));
        assert!(!any_segment_matches_api_key("theme")); // pure unrelated key
        assert!(!any_segment_matches_api_key("ui.markdown"));
    }
}
