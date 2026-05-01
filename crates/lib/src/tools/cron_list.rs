//! CronList tool: list stored cron routines.
//!
//! Returns a JSON array of routines with id, cron expression, next-run
//! and last-run timestamps, and enabled flag. Read-only — no permission
//! gate beyond the standard read-rule check.

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;
use crate::schedule::CronExpr;

use super::cron_support::open_store;

pub struct CronListTool;

#[derive(Serialize)]
struct RoutineSummary {
    id: String,
    name: String,
    cron_expression: String,
    enabled: bool,
    next_run_at: Option<String>,
    last_run_at: Option<String>,
}

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &'static str {
        "CronList"
    }

    fn description(&self) -> &'static str {
        "List stored cron routines with their cron expression, next-run and last-run timestamps, and enabled flag."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "include_disabled": {
                    "type": "boolean",
                    "default": true,
                    "description": "When false, omit routines whose enabled flag is false."
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let include_disabled = input
            .get("include_disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let store = open_store().map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to open schedule store: {e}"))
        })?;

        let now = Utc::now().naive_utc();
        let summaries: Vec<RoutineSummary> = store
            .list()
            .into_iter()
            .filter(|s| include_disabled || s.enabled)
            .map(|s| {
                let next = CronExpr::parse(&s.cron)
                    .ok()
                    .and_then(|c| c.next_after(&now))
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string());
                let last = s
                    .last_run_at
                    .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
                RoutineSummary {
                    id: s.name.clone(),
                    name: s.name,
                    cron_expression: s.cron,
                    enabled: s.enabled,
                    next_run_at: next,
                    last_run_at: last,
                }
            })
            .collect();

        let body = serde_json::to_string_pretty(&json!({
            "count": summaries.len(),
            "routines": summaries,
        }))
        .map_err(|e| ToolError::ExecutionFailed(format!("Serialize error: {e}")))?;

        Ok(ToolResult::success(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::Schedule;
    use crate::tools::cron_support::{test_ctx, with_test_store};
    use chrono::Utc;

    fn fixture(name: &str, cron: &str, enabled: bool) -> Schedule {
        Schedule {
            name: name.to_string(),
            cron: cron.to_string(),
            prompt: "x".to_string(),
            cwd: ".".to_string(),
            enabled,
            model: None,
            permission_mode: None,
            max_cost_usd: None,
            max_turns: None,
            created_at: Utc::now(),
            last_run_at: None,
            last_result: None,
            webhook_secret: None,
        }
    }

    #[tokio::test]
    async fn list_returns_all_routines_by_default() {
        let _guard = with_test_store();
        let store = open_store().unwrap();
        store.save(&fixture("alpha", "0 9 * * *", true)).unwrap();
        store.save(&fixture("beta", "0 10 * * *", false)).unwrap();

        let res = CronListTool.call(json!({}), &test_ctx()).await.unwrap();
        assert!(!res.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(parsed["count"].as_u64(), Some(2));
    }

    #[tokio::test]
    async fn list_can_skip_disabled() {
        let _guard = with_test_store();
        let store = open_store().unwrap();
        store.save(&fixture("alpha", "0 9 * * *", true)).unwrap();
        store.save(&fixture("beta", "0 10 * * *", false)).unwrap();

        let res = CronListTool
            .call(json!({"include_disabled": false}), &test_ctx())
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(parsed["count"].as_u64(), Some(1));
        assert_eq!(parsed["routines"][0]["id"].as_str(), Some("alpha"));
    }

    #[tokio::test]
    async fn list_includes_next_run_timestamp() {
        let _guard = with_test_store();
        let store = open_store().unwrap();
        store.save(&fixture("alpha", "*/5 * * * *", true)).unwrap();

        let res = CronListTool.call(json!({}), &test_ctx()).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&res.content).unwrap();
        let next = parsed["routines"][0]["next_run_at"].as_str().unwrap();
        assert!(next.ends_with('Z'));
    }

    #[tokio::test]
    async fn list_handles_empty_store() {
        let _guard = with_test_store();
        let res = CronListTool.call(json!({}), &test_ctx()).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(parsed["count"].as_u64(), Some(0));
        assert!(parsed["routines"].as_array().unwrap().is_empty());
    }
}
