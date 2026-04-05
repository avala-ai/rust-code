//! Agent Client Protocol (ACP) — stdio JSON-RPC server for IDE integrations.
//!
//! IDEs spawn `agent acp` as a subprocess and communicate via stdin/stdout
//! using newline-delimited JSON-RPC 2.0 messages. This enables VS Code,
//! Zed, JetBrains, and other editors to embed agent-code natively.
//!
//! # Protocol
//!
//! ```text
//! IDE                    agent acp
//!  │                        │
//!  │─── initialize ────────>│
//!  │<── result ─────────────│
//!  │                        │
//!  │─── message ───────────>│
//!  │<── events/text_delta ──│  (notification)
//!  │<── events/tool_start ──│  (notification)
//!  │<── events/tool_result ─│  (notification)
//!  │<── result ─────────────│  (final response)
//!  │                        │
//!  │─── shutdown ──────────>│
//!  │<── result ─────────────│
//!  └────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```bash
//! agent acp                 # Start ACP stdio server
//! agent acp --verbose       # With debug logging to stderr
//! ```

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use agent_code_lib::query::{QueryEngine, StreamSink};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

/// JSON-RPC 2.0 request from the IDE client.
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// JSON-RPC 2.0 response sent back to the IDE client.
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<serde_json::Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

// ---------------------------------------------------------------------------
// ACP StreamSink — bridges QueryEngine callbacks to JSON-RPC notifications
// ---------------------------------------------------------------------------

/// A [`StreamSink`] that sends JSON-RPC notifications over a channel.
///
/// Also accumulates text and tool names so the final `message` response
/// includes the complete response text and list of tools used.
///
/// Uses `std::sync::Mutex` (not tokio) because locks are held only
/// for brief string appends — no async work under the lock.
struct AcpSink {
    notify_tx: tokio::sync::mpsc::UnboundedSender<String>,
    text: std::sync::Mutex<String>,
    tools: std::sync::Mutex<Vec<String>>,
}

impl AcpSink {
    fn new(notify_tx: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        Self {
            notify_tx,
            text: std::sync::Mutex::new(String::new()),
            tools: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Send a JSON-RPC notification (no `id` field) to the IDE client.
    fn notify(&self, method: &str, params: serde_json::Value) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let _ = self
            .notify_tx
            .send(serde_json::to_string(&msg).unwrap_or_default());
    }
}

impl StreamSink for AcpSink {
    fn on_text(&self, text: &str) {
        if let Ok(mut t) = self.text.lock() {
            t.push_str(text);
        }
        self.notify("events/text_delta", serde_json::json!({ "text": text }));
    }

    fn on_tool_start(&self, name: &str, _input: &serde_json::Value) {
        if let Ok(mut tools) = self.tools.lock()
            && !tools.contains(&name.to_string())
        {
            tools.push(name.to_string());
        }
        self.notify("events/tool_start", serde_json::json!({ "name": name }));
    }

    fn on_tool_result(&self, name: &str, result: &agent_code_lib::tools::ToolResult) {
        self.notify(
            "events/tool_result",
            serde_json::json!({
                "name": name,
                "is_error": result.is_error,
            }),
        );
    }

    fn on_thinking(&self, text: &str) {
        self.notify("events/thinking", serde_json::json!({ "text": text }));
    }

    fn on_turn_complete(&self, turn: usize) {
        self.notify("events/turn_complete", serde_json::json!({ "turn": turn }));
    }

    fn on_error(&self, error: &str) {
        if let Ok(mut t) = self.text.lock() {
            t.push_str(&format!("\n[Error: {error}]"));
        }
        self.notify("events/error", serde_json::json!({ "message": error }));
    }

    fn on_usage(&self, usage: &agent_code_lib::llm::message::Usage) {
        self.notify(
            "events/usage",
            serde_json::json!({
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
            }),
        );
    }

    fn on_compact(&self, freed_tokens: u64) {
        self.notify(
            "events/compact",
            serde_json::json!({ "freed_tokens": freed_tokens }),
        );
    }

    fn on_warning(&self, msg: &str) {
        self.notify("events/warning", serde_json::json!({ "message": msg }));
    }
}

// ---------------------------------------------------------------------------
// Request handlers
// ---------------------------------------------------------------------------

/// Handle the `initialize` handshake method.
fn handle_initialize(id: Option<serde_json::Value>) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "name": "agent-code",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol_version": "1",
            "capabilities": {
                "streaming": true,
                "tools": true,
                "thinking": true,
            }
        }),
    )
}

/// Handle the `message` method — run a full agent turn.
async fn handle_message(
    id: Option<serde_json::Value>,
    params: &serde_json::Value,
    engine: &Arc<Mutex<QueryEngine>>,
    notify_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> JsonRpcResponse {
    let content = match params.get("content").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                -32602,
                "Invalid params: missing 'content' string".to_string(),
            );
        }
    };

    let sink = Arc::new(AcpSink::new(notify_tx.clone()));
    let sink_ref: &dyn StreamSink = &*sink;

    let mut engine = engine.lock().await;
    let turn_result = engine.run_turn_with_sink(&content, sink_ref).await;

    let response_text = sink.text.lock().map(|t| t.clone()).unwrap_or_default();
    let tools_used = sink.tools.lock().map(|t| t.clone()).unwrap_or_default();
    let state = engine.state();
    let turn_count = state.turn_count;
    let cost_usd = state.total_cost_usd;

    if let Err(e) = turn_result {
        return JsonRpcResponse::error(id, -32000, format!("Turn failed: {e}"));
    }

    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "response": response_text,
            "turn_count": turn_count,
            "tools_used": tools_used,
            "cost_usd": cost_usd,
        }),
    )
}

/// Handle the `status` method.
async fn handle_status(
    id: Option<serde_json::Value>,
    engine: &Arc<Mutex<QueryEngine>>,
) -> JsonRpcResponse {
    let engine = engine.lock().await;
    let s = engine.state();

    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "session_id": s.session_id,
            "model": s.config.api.model,
            "cwd": s.cwd,
            "turn_count": s.turn_count,
            "message_count": s.messages.len(),
            "cost_usd": s.total_cost_usd,
            "plan_mode": s.plan_mode,
        }),
    )
}

/// Handle the `cancel` method — cancel the current turn.
async fn handle_cancel(
    id: Option<serde_json::Value>,
    engine: &Arc<Mutex<QueryEngine>>,
) -> JsonRpcResponse {
    let engine = engine.lock().await;
    engine.cancel();

    JsonRpcResponse::success(id, serde_json::json!({ "cancelled": true }))
}

// ---------------------------------------------------------------------------
// Write helper
// ---------------------------------------------------------------------------

/// Serialize and write a JSON-RPC message to stdout (one line).
fn write_to_stdout(msg: &str) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(msg.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

/// Write a JSON-RPC response to stdout.
fn write_response(resp: &JsonRpcResponse) {
    let msg = serde_json::to_string(resp).unwrap_or_default();
    write_to_stdout(&msg);
}

// ---------------------------------------------------------------------------
// Main ACP entry point
// ---------------------------------------------------------------------------

/// Start the ACP stdio JSON-RPC server.
///
/// Reads newline-delimited JSON-RPC 2.0 requests from stdin, dispatches
/// them to the appropriate handler, and writes responses/notifications to
/// stdout. All diagnostic output goes to stderr via tracing.
pub async fn run_acp(engine: QueryEngine) -> anyhow::Result<()> {
    let engine = Arc::new(Mutex::new(engine));

    // Channel for incoming parsed requests from the stdin reader thread.
    let (req_tx, mut req_rx) = tokio::sync::mpsc::unbounded_channel::<JsonRpcRequest>();

    // Channel for outgoing notifications (sent during message processing).
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Stdin reader — runs in a dedicated OS thread because std::io::stdin
    // is blocking and cannot be used with tokio's async I/O.
    let req_tx_clone = req_tx.clone();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break, // stdin closed
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(req) => {
                    if req_tx_clone.send(req).is_err() {
                        break; // receiver dropped, shutting down
                    }
                }
                Err(e) => {
                    let err = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
                    let msg = serde_json::to_string(&err).unwrap_or_default();
                    write_to_stdout(&msg);
                }
            }
        }
    });

    // Notification writer — receives serialized JSON-RPC notification
    // strings and writes them to stdout. Runs as an async task so
    // notifications can be sent concurrently with request processing.
    tokio::spawn(async move {
        while let Some(msg) = notify_rx.recv().await {
            write_to_stdout(&msg);
        }
    });

    // Main request dispatch loop.
    while let Some(req) = req_rx.recv().await {
        let response = match req.method.as_str() {
            "initialize" => handle_initialize(req.id),
            "message" => handle_message(req.id, &req.params, &engine, &notify_tx).await,
            "status" => handle_status(req.id, &engine).await,
            "cancel" => handle_cancel(req.id, &engine).await,
            "shutdown" => {
                let resp = JsonRpcResponse::success(req.id, serde_json::json!({ "ok": true }));
                write_response(&resp);
                break;
            }
            other => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {other}")),
        };
        write_response(&response);
    }

    Ok(())
}
