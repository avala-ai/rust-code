//! Interactive REPL (Read-Eval-Print Loop).
//!
//! The main user interaction loop. Reads input via rustyline,
//! passes it to the query engine, and streams output to the terminal.

use std::io::Write;
use std::sync::{Arc, Mutex};

use crossterm::style::Stylize;
use rustyline::error::ReadlineError;

use crate::llm::message::Usage;
use crate::query::{QueryEngine, StreamSink};
use crate::tools::ToolResult;

/// Stream sink that writes directly to the terminal.
struct TerminalSink {
    /// Tracks whether we're mid-line (for proper newline handling).
    mid_line: Arc<Mutex<bool>>,
}

impl TerminalSink {
    fn new() -> Self {
        Self {
            mid_line: Arc::new(Mutex::new(false)),
        }
    }

    fn ensure_newline(&self) {
        let mut mid = self.mid_line.lock().unwrap();
        if *mid {
            println!();
            *mid = false;
        }
    }
}

impl StreamSink for TerminalSink {
    fn on_text(&self, text: &str) {
        print!("{text}");
        let _ = std::io::stdout().flush();
        *self.mid_line.lock().unwrap() = !text.ends_with('\n');
    }

    fn on_tool_start(&self, tool_name: &str, input: &serde_json::Value) {
        self.ensure_newline();
        let label = format!(" {tool_name} ");
        let detail = summarize_tool_input(tool_name, input);
        eprintln!(
            "{} {}",
            label.on_dark_cyan().white().bold(),
            detail.dark_grey()
        );
    }

    fn on_tool_result(&self, tool_name: &str, result: &ToolResult) {
        if result.is_error {
            let label = format!(" {tool_name} ERROR ");
            eprintln!(
                "{} {}",
                label.on_red().white().bold(),
                result.content.lines().next().unwrap_or("").red()
            );
        }
    }

    fn on_thinking(&self, _text: &str) {
        // Thinking is hidden by default.
    }

    fn on_turn_complete(&self, _turn: usize) {
        self.ensure_newline();
    }

    fn on_error(&self, error: &str) {
        self.ensure_newline();
        eprintln!("{} {error}", " ERROR ".on_red().white().bold());
    }

    fn on_usage(&self, usage: &Usage) {
        let total = usage.total();
        if total > 0 {
            let _ = total; // Usage display is optional in verbose mode.
        }
    }
}

/// Run the interactive REPL loop.
pub async fn run_repl(engine: &mut QueryEngine) -> anyhow::Result<()> {
    // Configure editing mode (vi if EDITOR contains "vi", else emacs).
    let input_mode = super::keymap::InputMode::default();
    let rl_config = rustyline::Config::builder()
        .edit_mode(input_mode.to_edit_mode())
        .build();
    let mut rl =
        rustyline::Editor::<(), rustyline::history::DefaultHistory>::with_config(rl_config)?;

    // Generate a session ID for persistence.
    let session_id = crate::services::session::new_session_id();

    // Load history.
    let history_path = dirs::data_dir().map(|d| d.join("rust-code").join("history.txt"));
    if let Some(ref path) = history_path {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = rl.load_history(path);
    }

    // Welcome message.
    println!(
        "{} {}\n{}\n",
        " rc ".on_dark_cyan().white().bold(),
        format!("session {session_id}").dark_grey(),
        "Type your message, or /help for commands. Ctrl+C to cancel, Ctrl+D to exit.".dark_grey(),
    );

    let sink = TerminalSink::new();

    loop {
        let prompt = format!("{} ", ">".dark_cyan().bold(),);

        match rl.readline(&prompt) {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                rl.add_history_entry(input)?;

                // Handle slash commands.
                if input.starts_with('/') {
                    match crate::commands::execute(input, engine) {
                        crate::commands::CommandResult::Handled => continue,
                        crate::commands::CommandResult::Exit => break,
                        crate::commands::CommandResult::Passthrough(text) => {
                            sink.ensure_newline();
                            if let Err(e) = engine.run_turn_with_sink(&text, &sink).await {
                                eprintln!("{} {e}", " ERROR ".on_red().white().bold());
                            }
                            sink.ensure_newline();
                            println!();
                        }
                        crate::commands::CommandResult::Prompt(prompt) => {
                            sink.ensure_newline();
                            if let Err(e) = engine.run_turn_with_sink(&prompt, &sink).await {
                                eprintln!("{} {e}", " ERROR ".on_red().white().bold());
                            }
                            sink.ensure_newline();
                            println!();
                        }
                    }
                    continue;
                }

                // Run the agent turn.
                sink.ensure_newline();
                if let Err(e) = engine.run_turn_with_sink(input, &sink).await {
                    eprintln!("{} {e}", " ERROR ".on_red().white().bold());
                }
                sink.ensure_newline();
                println!();
            }
            Err(ReadlineError::Interrupted) => {
                engine.cancel();
                eprintln!("{}", "(cancelled)".dark_grey());
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                eprintln!("Input error: {e}");
                break;
            }
        }
    }

    // Save history.
    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }

    // Persist session for later resume.
    let state = engine.state();
    if !state.messages.is_empty() {
        match crate::services::session::save_session(
            &session_id,
            &state.messages,
            &state.cwd,
            &state.config.api.model,
            state.turn_count,
        ) {
            Ok(_) => {}
            Err(e) => eprintln!("{}", format!("Failed to save session: {e}").dark_grey()),
        }
    }

    // Print session summary.
    if state.total_usage.total() > 0 {
        println!(
            "\n{} {} turns, {} tokens, ${:.4}",
            " Session ".on_dark_cyan().white().bold(),
            state.turn_count,
            state.total_usage.total(),
            state.total_cost_usd,
        );
    }

    Ok(())
}

/// Create a short summary of tool input for display.
fn summarize_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "FileRead" | "FileWrite" | "FileEdit" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Grep" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Glob" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => serde_json::to_string(input).unwrap_or_default(),
    }
}
