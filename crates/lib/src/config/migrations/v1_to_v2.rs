//! v1 → v2: rename `api.token` to `api.api_key`.
//!
//! Early prototypes of the JSON settings file used `api.token` to hold
//! the LLM credential before the codebase standardized on `api.api_key`
//! everywhere else (env vars, the typed struct, the helper command).
//! This migration consolidates the two so older files keep working
//! after the rename.
//!
//! Behavior:
//! - `api.token` present, `api.api_key` absent → move the value over.
//! - Both present → keep `api.api_key` (it's the canonical field) and
//!   drop `api.token` so the duplicate doesn't drift later.
//! - Neither present → no-op.
//! - `api` missing or not an object → no-op.

use anyhow::Result;

use super::Migration;

/// Second migration: consolidates the legacy `api.token` field into
/// `api.api_key`. Demonstrates the "rename" pattern for future schema
/// changes that need to move a value between keys.
pub struct RenameApiTokenToApiKey;

impl Migration for RenameApiTokenToApiKey {
    fn from_version(&self) -> u32 {
        1
    }

    fn to_version(&self) -> u32 {
        2
    }

    fn description(&self) -> &'static str {
        "rename api.token to api.api_key"
    }

    fn migrate(&self, value: &mut serde_json::Value) -> Result<()> {
        let Some(root) = value.as_object_mut() else {
            return Ok(());
        };
        let Some(api) = root.get_mut("api").and_then(|v| v.as_object_mut()) else {
            return Ok(());
        };

        let legacy = api.remove("token");
        if let Some(legacy_value) = legacy {
            api.entry("api_key".to_string()).or_insert(legacy_value);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn moves_token_into_api_key_when_only_token_present() {
        let m = RenameApiTokenToApiKey;
        let mut v = json!({"api": {"token": "sk-legacy", "model": "x"}});
        m.migrate(&mut v).unwrap();
        assert_eq!(v["api"]["api_key"], json!("sk-legacy"));
        assert!(v["api"].get("token").is_none());
    }

    #[test]
    fn keeps_existing_api_key_when_both_present() {
        let m = RenameApiTokenToApiKey;
        let mut v = json!({"api": {"token": "sk-legacy", "api_key": "sk-canonical"}});
        m.migrate(&mut v).unwrap();
        assert_eq!(v["api"]["api_key"], json!("sk-canonical"));
        assert!(v["api"].get("token").is_none());
    }

    #[test]
    fn no_op_when_neither_field_present() {
        let m = RenameApiTokenToApiKey;
        let mut v = json!({"api": {"model": "x"}});
        let before = v.clone();
        m.migrate(&mut v).unwrap();
        assert_eq!(v, before);
    }

    #[test]
    fn no_op_when_api_section_missing() {
        let m = RenameApiTokenToApiKey;
        let mut v = json!({"ui": {"theme": "dark"}});
        let before = v.clone();
        m.migrate(&mut v).unwrap();
        assert_eq!(v, before);
    }

    #[test]
    fn no_op_when_api_not_an_object() {
        let m = RenameApiTokenToApiKey;
        let mut v = json!({"api": "scalar"});
        let before = v.clone();
        m.migrate(&mut v).unwrap();
        assert_eq!(v, before);
    }
}
