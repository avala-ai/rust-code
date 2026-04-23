//! MCP client: high-level interface for MCP server interactions.
//!
//! Manages the connection lifecycle, tool discovery, and tool
//! execution for a single MCP server.

use std::time::Duration;

use tracing::{debug, info, warn};

use super::transport::McpTransportConnection;
use super::types::*;

/// Maximum backoff delay between reconnect attempts.
const MAX_BACKOFF_MS: u64 = 30_000;

/// Initial delay before the first reconnect retry.
const INITIAL_BACKOFF_MS: u64 = 1_000;

/// Compute the exponential-backoff delay for the given attempt number
/// (0-indexed). Doubles each attempt, capped at [`MAX_BACKOFF_MS`].
///
/// Kept as a pure function so the schedule can be exercised in unit
/// tests without actually sleeping or opening a transport.
pub(crate) fn backoff_delay_ms(attempt: u32) -> u64 {
    INITIAL_BACKOFF_MS
        .saturating_mul(1u64 << attempt.min(20))
        .min(MAX_BACKOFF_MS)
}

/// Client for a single MCP server connection.
pub struct McpClient {
    /// Server configuration.
    config: McpServerConfig,
    /// Transport connection.
    transport: Option<McpTransportConnection>,
    /// Discovered tools.
    tools: Vec<McpTool>,
    /// Discovered resources.
    resources: Vec<McpResource>,
    /// Connection status.
    status: McpConnectionStatus,
}

impl McpClient {
    /// Create a new client for the given server configuration.
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            transport: None,
            tools: Vec::new(),
            resources: Vec::new(),
            status: McpConnectionStatus::Disconnected,
        }
    }

    /// Get the server name.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Get the connection status.
    pub fn status(&self) -> &McpConnectionStatus {
        &self.status
    }

    /// Get discovered tools.
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Get discovered resources.
    pub fn resources(&self) -> &[McpResource] {
        &self.resources
    }

    /// Connect to the MCP server and perform initialization.
    pub async fn connect(&mut self) -> Result<(), String> {
        self.status = McpConnectionStatus::Connecting;

        let transport = match &self.config.transport {
            McpTransport::Stdio { command, args } => {
                McpTransportConnection::connect_stdio(command, args, &self.config.env).await?
            }
            McpTransport::Sse { url } => McpTransportConnection::connect_sse(url).await?,
        };

        // Initialize the connection.
        let init_result = transport
            .request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                        "resources": {}
                    },
                    "clientInfo": {
                        "name": "agent-code",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
            )
            .await?;

        debug!("MCP server initialized: {:?}", init_result);

        // Send initialized notification.
        transport.notify("notifications/initialized", None).await?;

        self.transport = Some(transport);
        self.status = McpConnectionStatus::Connected;

        // Discover tools and resources.
        self.discover_tools().await?;
        self.discover_resources().await?;

        info!(
            "MCP server '{}' connected: {} tools, {} resources",
            self.config.name,
            self.tools.len(),
            self.resources.len()
        );

        Ok(())
    }

    /// Discover available tools from the server.
    async fn discover_tools(&mut self) -> Result<(), String> {
        let transport = self.transport.as_ref().ok_or("Not connected")?;

        let result = transport.request("tools/list", None).await?;

        if let Some(tools) = result.get("tools").and_then(|v| v.as_array()) {
            self.tools = tools
                .iter()
                .filter_map(|t| serde_json::from_value(t.clone()).ok())
                .collect();
        }

        Ok(())
    }

    /// Discover available resources from the server.
    async fn discover_resources(&mut self) -> Result<(), String> {
        let transport = self.transport.as_ref().ok_or("Not connected")?;

        match transport.request("resources/list", None).await {
            Ok(result) => {
                if let Some(resources) = result.get("resources").and_then(|v| v.as_array()) {
                    self.resources = resources
                        .iter()
                        .filter_map(|r| serde_json::from_value(r.clone()).ok())
                        .collect();
                }
            }
            Err(e) => {
                // Resources are optional — server may not support them.
                debug!(
                    "MCP server '{}' doesn't support resources: {e}",
                    self.config.name
                );
            }
        }

        Ok(())
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, String> {
        let transport = self.transport.as_ref().ok_or("Not connected")?;

        let result = transport
            .request(
                "tools/call",
                Some(serde_json::json!({
                    "name": tool_name,
                    "arguments": arguments,
                })),
            )
            .await?;

        serde_json::from_value(result).map_err(|e| format!("Invalid tool result: {e}"))
    }

    /// Read a resource from the MCP server.
    pub async fn read_resource(&self, uri: &str) -> Result<String, String> {
        let transport = self.transport.as_ref().ok_or("Not connected")?;

        let result = transport
            .request("resources/read", Some(serde_json::json!({ "uri": uri })))
            .await?;

        // Extract text content from the response.
        if let Some(contents) = result.get("contents").and_then(|v| v.as_array()) {
            let text: Vec<&str> = contents
                .iter()
                .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                .collect();
            Ok(text.join("\n"))
        } else {
            Ok(result.to_string())
        }
    }

    /// Disconnect from the MCP server.
    pub async fn disconnect(&mut self) {
        if let Some(transport) = self.transport.take() {
            transport.shutdown().await;
        }
        self.tools.clear();
        self.resources.clear();
        self.status = McpConnectionStatus::Disconnected;
    }

    /// Attempt to reconnect to the MCP server with exponential backoff.
    ///
    /// Drops the current transport (if any) and retries [`connect`] up
    /// to `max_attempts` times. Between attempts the client sleeps for
    /// a delay that doubles each try, capped at 30 s. Returns the last
    /// error if every attempt fails.
    ///
    /// Use this after a transient transport failure (subprocess exit,
    /// network hiccup) to restore tool discovery without asking the
    /// user to restart the session.
    pub async fn reconnect_with_backoff(&mut self, max_attempts: u32) -> Result<(), String> {
        if max_attempts == 0 {
            return Err("max_attempts must be greater than zero".to_string());
        }

        // Drop the stale transport so `connect` can install a fresh one.
        if let Some(transport) = self.transport.take() {
            transport.shutdown().await;
        }
        self.tools.clear();
        self.resources.clear();

        let mut last_err = String::new();
        for attempt in 0..max_attempts {
            if attempt > 0 {
                let delay_ms = backoff_delay_ms(attempt - 1);
                debug!(
                    "MCP '{}' reconnect attempt {}/{} — waiting {} ms",
                    self.config.name,
                    attempt + 1,
                    max_attempts,
                    delay_ms
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            match self.connect().await {
                Ok(()) => {
                    info!(
                        "MCP '{}' reconnected on attempt {}",
                        self.config.name,
                        attempt + 1
                    );
                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        "MCP '{}' reconnect attempt {} failed: {}",
                        self.config.name,
                        attempt + 1,
                        e
                    );
                    last_err = e;
                }
            }
        }

        self.status = McpConnectionStatus::Error(last_err.clone());
        Err(format!(
            "MCP '{}' failed to reconnect after {max_attempts} attempts: {last_err}",
            self.config.name
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule_doubles_until_cap() {
        // 1s → 2s → 4s → 8s → 16s → 30s (capped) → 30s …
        assert_eq!(backoff_delay_ms(0), 1_000);
        assert_eq!(backoff_delay_ms(1), 2_000);
        assert_eq!(backoff_delay_ms(2), 4_000);
        assert_eq!(backoff_delay_ms(3), 8_000);
        assert_eq!(backoff_delay_ms(4), 16_000);
        assert_eq!(backoff_delay_ms(5), MAX_BACKOFF_MS);
        assert_eq!(backoff_delay_ms(10), MAX_BACKOFF_MS);
        // Huge attempt counts must not panic from shift overflow.
        assert_eq!(backoff_delay_ms(u32::MAX), MAX_BACKOFF_MS);
    }

    #[test]
    fn backoff_is_monotonic_non_decreasing() {
        let mut prev = 0;
        for a in 0..40 {
            let d = backoff_delay_ms(a);
            assert!(d >= prev, "backoff decreased at attempt {a}: {prev} -> {d}");
            prev = d;
        }
    }

    #[tokio::test]
    async fn reconnect_zero_attempts_is_rejected() {
        let mut client = McpClient::new(McpServerConfig {
            transport: McpTransport::Stdio {
                command: "/bin/false".to_string(),
                args: Vec::new(),
            },
            name: "test".to_string(),
            env: Default::default(),
        });
        let err = client.reconnect_with_backoff(0).await.unwrap_err();
        assert!(
            err.contains("max_attempts"),
            "expected max_attempts rejection, got: {err}"
        );
    }
}
