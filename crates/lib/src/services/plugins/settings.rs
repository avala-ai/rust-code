//! Per-project plugin enable/disable persistence.
//!
//! Lives at `<project>/.agent/plugin-settings.toml`. Atomic writes are
//! routed through [`crate::config::atomic::atomic_write_secret`] so a
//! crash mid-write never leaves the file half-truncated and so the
//! captured `0600` mode is preserved across rewrites.
//!
//! The file shape is intentionally tiny:
//!
//! ```toml
//! [plugin."@builtin/foo"]
//! enabled = false
//!
//! [plugin."my-plugin"]
//! enabled = true
//! ```

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::atomic::{AtomicWriteError, atomic_write_secret};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PluginSettings {
    #[serde(default)]
    plugin: HashMap<String, PluginEntry>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl PluginSettings {
    /// Load from the given path. Missing file → empty settings.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw =
            std::fs::read_to_string(path).map_err(|e| format!("read plugin-settings: {e}"))?;
        toml::from_str(&raw).map_err(|e| format!("parse plugin-settings: {e}"))
    }

    pub fn enabled_for(&self, id: &str) -> Option<bool> {
        self.plugin.get(id).and_then(|e| e.enabled)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        self.plugin.entry(id.to_string()).or_default().enabled = Some(enabled);
    }

    /// Serialise and atomically write to disk.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let body = toml::to_string_pretty(self).map_err(|e| format!("serialise: {e}"))?;
        atomic_write_secret(path, body.as_bytes()).map_err(|e: AtomicWriteError| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_enable_disable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin-settings.toml");

        let mut s = PluginSettings::load(&path).unwrap();
        assert_eq!(s.enabled_for("foo"), None);

        s.set_enabled("foo", false);
        s.set_enabled("@builtin/bar", true);
        s.save(&path).unwrap();

        let loaded = PluginSettings::load(&path).unwrap();
        assert_eq!(loaded.enabled_for("foo"), Some(false));
        assert_eq!(loaded.enabled_for("@builtin/bar"), Some(true));
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let s = PluginSettings::load(&dir.path().join("absent.toml")).unwrap();
        assert!(s.enabled_for("anything").is_none());
    }
}
