//! Permission denial tracking.
//!
//! Records which tool calls were denied and why, for reporting
//! to the user and SDK consumers.

use std::collections::VecDeque;

/// A recorded permission denial event.
#[derive(Debug, Clone)]
pub struct DenialRecord {
    /// Tool that was denied.
    pub tool_name: String,
    /// The tool_use ID from the model.
    pub tool_use_id: String,
    /// Reason for denial.
    pub reason: String,
    /// Timestamp of the denial.
    pub timestamp: String,
    /// Summary of what the tool was trying to do.
    pub input_summary: String,
}

/// Tracks permission denials for the current session.
pub struct DenialTracker {
    /// Recent denials (bounded to prevent unbounded growth).
    records: VecDeque<DenialRecord>,
    /// Maximum number of denials to retain.
    max_records: usize,
    /// Total denials this session (even if records were evicted).
    total_denials: usize,
}

impl DenialTracker {
    pub fn new(max_records: usize) -> Self {
        Self {
            records: VecDeque::new(),
            max_records,
            total_denials: 0,
        }
    }

    /// Record a new denial.
    pub fn record(
        &mut self,
        tool_name: &str,
        tool_use_id: &str,
        reason: &str,
        input: &serde_json::Value,
    ) {
        let summary = summarize_input(tool_name, input);

        self.records.push_back(DenialRecord {
            tool_name: tool_name.to_string(),
            tool_use_id: tool_use_id.to_string(),
            reason: reason.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            input_summary: summary,
        });

        self.total_denials += 1;

        // Evict oldest if over limit.
        while self.records.len() > self.max_records {
            self.records.pop_front();
        }
    }

    /// Get all recorded denials.
    pub fn denials(&self) -> &VecDeque<DenialRecord> {
        &self.records
    }

    /// Total denials this session.
    pub fn total(&self) -> usize {
        self.total_denials
    }

    /// Clear all records.
    pub fn clear(&mut self) {
        self.records.clear();
        self.total_denials = 0;
    }

    /// Get denials for a specific tool.
    pub fn denials_for_tool(&self, tool_name: &str) -> Vec<&DenialRecord> {
        self.records
            .iter()
            .filter(|r| r.tool_name == tool_name)
            .collect()
    }
}

fn summarize_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(100)
            .collect(),
        "FileWrite" | "FileEdit" | "FileRead" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => serde_json::to_string(input)
            .unwrap_or_default()
            .chars()
            .take(100)
            .collect(),
    }
}
