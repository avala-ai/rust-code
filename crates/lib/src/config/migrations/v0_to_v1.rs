//! v0 → v1: stamp the schema version.
//!
//! v0 is the implicit baseline — files that predate the migration
//! framework simply have no `schema_version` field. This step makes
//! that explicit by recording `"schema_version": 1`. The runner then
//! has a stable anchor for any future structural change.
//!
//! The migration touches no other field, so an in-flight v0 settings
//! file gains version stamping with zero behavioral impact.

use anyhow::Result;

use super::Migration;

/// First migration: stamps `"schema_version": 1` onto a v0 settings
/// file without altering any other field.
pub struct StampVersion;

impl Migration for StampVersion {
    fn from_version(&self) -> u32 {
        0
    }

    fn to_version(&self) -> u32 {
        1
    }

    fn description(&self) -> &'static str {
        "stamp schema_version onto pre-versioned settings files"
    }

    fn migrate(&self, _value: &mut serde_json::Value) -> Result<()> {
        // No structural change; the runner stamps the version field
        // after a successful migrate().
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_to_versions_are_zero_and_one() {
        let m = StampVersion;
        assert_eq!(m.from_version(), 0);
        assert_eq!(m.to_version(), 1);
    }

    #[test]
    fn migrate_does_not_touch_non_version_fields() {
        let m = StampVersion;
        let mut v = json!({
            "api": {"model": "test", "timeout_secs": 60},
            "permissions": {"default_mode": "ask"}
        });
        let before = v.clone();
        m.migrate(&mut v).unwrap();
        assert_eq!(v, before);
    }
}
