//! Query engine: the core agent loop.
//!
//! Implements the agentic cycle:
//!
//! 1. Build system prompt + conversation history
//! 2. Call LLM with streaming
//! 3. Accumulate response content blocks
//! 4. Extract tool_use blocks
//! 5. Execute tools (concurrent/serial batching)
//! 6. Inject tool results into history
//! 7. Repeat from step 2 until no tool_use or max turns
//!
//! This is the heart of the agent. All tool execution, permission
//! checking, and streaming coordination happens here.

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::LlmError;
use crate::llm::client::{CompletionRequest, LlmClient};
use crate::llm::message::*;
use crate::llm::stream::StreamEvent;
use crate::permissions::{PermissionChecker, PermissionDecision};
use crate::state::AppState;
use crate::tools::executor::{execute_tool_calls, extract_tool_calls, ToolCallResult};
use crate::tools::registry::ToolRegistry;
use crate::tools::ToolContext;

/// Configuration for the query engine.
pub struct QueryEngineConfig {
    pub max_turns: Option<usize>,
    pub verbose: bool,
}

/// The query engine orchestrates the agent loop.
pub struct QueryEngine {
    llm: LlmClient,
    tools: ToolRegistry,
    permissions: Arc<PermissionChecker>,
    state: AppState,
    config: QueryEngineConfig,
    cancel: CancellationToken,
}

/// Callback for streaming events to the UI.
pub trait StreamSink: Send + Sync {
    fn on_text(&self, text: &str);
    fn on_tool_start(&self, tool_name: &str, input: &serde_json::Value);
    fn on_tool_result(&self, tool_name: &str, result: &crate::tools::ToolResult);
    fn on_thinking(&self, _text: &str) {}
    fn on_turn_complete(&self, _turn: usize) {}
    fn on_error(&self, error: &str);
    fn on_usage(&self, _usage: &Usage) {}
}

/// A no-op stream sink for non-interactive mode.
pub struct NullSink;
impl StreamSink for NullSink {
    fn on_text(&self, _: &str) {}
    fn on_tool_start(&self, _: &str, _: &serde_json::Value) {}
    fn on_tool_result(&self, _: &str, _: &crate::tools::ToolResult) {}
    fn on_error(&self, _: &str) {}
}

impl QueryEngine {
    pub fn new(
        llm: LlmClient,
        tools: ToolRegistry,
        permissions: PermissionChecker,
        state: AppState,
        config: QueryEngineConfig,
    ) -> Self {
        Self {
            llm,
            tools,
            permissions: Arc::new(permissions),
            state,
            config,
            cancel: CancellationToken::new(),
        }
    }

    /// Get a reference to the app state.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Get a mutable reference to the app state.
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    /// Run a single turn: process user input through the full agent loop.
    pub async fn run_turn(&mut self, user_input: &str) -> crate::error::Result<()> {
        self.run_turn_with_sink(user_input, &NullSink).await
    }

    /// Run a turn with a stream sink for real-time UI updates.
    pub async fn run_turn_with_sink(
        &mut self,
        user_input: &str,
        sink: &dyn StreamSink,
    ) -> crate::error::Result<()> {
        // Add the user message to history.
        let user_msg = user_message(user_input);
        self.state.push_message(user_msg);

        let max_turns = self.config.max_turns.unwrap_or(50);

        // Agent loop: call LLM → execute tools → repeat.
        for turn in 0..max_turns {
            self.state.turn_count = turn + 1;
            self.state.is_query_active = true;

            debug!("Agent turn {}/{}", turn + 1, max_turns);

            // Build the API request.
            let system_prompt = build_system_prompt(&self.tools, &self.state);
            let tool_schemas = self.tools.schemas();

            let request = CompletionRequest {
                messages: self.state.history(),
                system_prompt: &system_prompt,
                tools: &tool_schemas,
                max_tokens: self.state.config.api.max_output_tokens,
            };

            // Stream the response.
            let mut rx = match self.llm.stream_completion(request).await {
                Ok(rx) => rx,
                Err(e) => {
                    // Handle retryable errors.
                    if let LlmError::RateLimited { retry_after_ms } = &e {
                        warn!("Rate limited, waiting {}ms", retry_after_ms);
                        tokio::time::sleep(
                            std::time::Duration::from_millis(*retry_after_ms),
                        )
                        .await;
                        continue;
                    }
                    sink.on_error(&e.to_string());
                    self.state.is_query_active = false;
                    return Err(e.into());
                }
            };

            // Accumulate content blocks from the stream.
            let mut content_blocks = Vec::new();
            let mut usage = Usage::default();

            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::TextDelta(text) => {
                        sink.on_text(&text);
                    }
                    StreamEvent::ContentBlockComplete(block) => {
                        // Notify sink about tool starts.
                        if let ContentBlock::ToolUse { ref name, ref input, .. } = block {
                            sink.on_tool_start(name, input);
                        }
                        if let ContentBlock::Thinking { ref thinking, .. } = block {
                            sink.on_thinking(thinking);
                        }
                        content_blocks.push(block);
                    }
                    StreamEvent::Done {
                        usage: u,
                        stop_reason: _,
                    } => {
                        usage = u;
                        sink.on_usage(&usage);
                    }
                    StreamEvent::Error(msg) => {
                        sink.on_error(&msg);
                    }
                    _ => {}
                }
            }

            // Record the assistant message.
            let assistant_msg = Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                content: content_blocks.clone(),
                model: Some(self.state.config.api.model.clone()),
                usage: Some(usage.clone()),
                stop_reason: None,
                request_id: None,
            });
            self.state.push_message(assistant_msg);
            self.state.record_usage(&usage, &self.state.config.api.model.clone());

            // Extract tool calls from the response.
            let tool_calls = extract_tool_calls(&content_blocks);

            if tool_calls.is_empty() {
                // No tools requested — turn is complete.
                info!("Turn complete (no tool calls)");
                sink.on_turn_complete(turn + 1);
                self.state.is_query_active = false;
                return Ok(());
            }

            // Execute tool calls.
            info!("Executing {} tool call(s)", tool_calls.len());
            let cwd = PathBuf::from(&self.state.cwd);
            let tool_ctx = ToolContext {
                cwd,
                cancel: self.cancel.clone(),
                permission_checker: self.permissions.clone(),
                verbose: self.config.verbose,
            };

            let results = execute_tool_calls(
                &tool_calls,
                self.tools.all(),
                &tool_ctx,
                &self.permissions,
            )
            .await;

            // Inject tool results as user messages.
            for result in &results {
                sink.on_tool_result(&result.tool_name, &result.result);
                let msg = tool_result_message(
                    &result.tool_use_id,
                    &result.result.content,
                    result.result.is_error,
                );
                self.state.push_message(msg);
            }

            // Continue the loop — the model will see the tool results.
        }

        warn!("Max turns ({max_turns}) reached");
        self.state.is_query_active = false;
        Ok(())
    }

    /// Cancel the current operation.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// Build the system prompt from tool definitions and app state.
pub fn build_system_prompt(tools: &ToolRegistry, state: &AppState) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        "You are an AI coding agent. You help users with software engineering tasks \
         by reading, writing, and searching code. Use the tools available to you to \
         accomplish tasks.\n\n",
    );

    // Environment context.
    prompt.push_str(&format!(
        "# Environment\n\
         - Working directory: {}\n\
         - Platform: {}\n\
         - Shell: bash\n\n",
        state.cwd,
        std::env::consts::OS,
    ));

    // Tool documentation.
    prompt.push_str("# Available Tools\n\n");
    for tool in tools.all() {
        if tool.is_enabled() {
            prompt.push_str(&format!("## {}\n{}\n\n", tool.name(), tool.prompt()));
        }
    }

    // Safety guidelines.
    prompt.push_str(
        "# Guidelines\n\
         - Read files before editing them.\n\
         - Prefer editing existing files over creating new ones.\n\
         - Use the appropriate dedicated tool instead of shell commands when possible.\n\
         - Be careful not to introduce security vulnerabilities.\n\
         - Only make changes that were requested.\n",
    );

    prompt
}
