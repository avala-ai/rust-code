//! Slash command system.
//!
//! Commands are user-invokable actions triggered by `/command` syntax
//! in the REPL. They can be:
//!
//! - **Built-in**: implemented directly in Rust
//! - **Skills**: loaded from skill files, executed as prompt templates
//!
//! Commands have access to the query engine state and can modify
//! the conversation, change settings, or execute side effects.

use crate::query::QueryEngine;

/// Result of executing a command.
pub enum CommandResult {
    /// Command handled, continue REPL.
    Handled,
    /// Exit the REPL.
    Exit,
    /// Not a command — pass through as a prompt to the agent.
    Passthrough(String),
    /// Command wants to inject a prompt for the agent.
    Prompt(String),
}

/// A built-in command definition.
pub struct Command {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub hidden: bool,
}

/// All built-in commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "help",
        aliases: &["h", "?"],
        description: "Show available commands",
        hidden: false,
    },
    Command {
        name: "exit",
        aliases: &["quit", "q"],
        description: "Exit the REPL",
        hidden: false,
    },
    Command {
        name: "clear",
        aliases: &[],
        description: "Clear conversation history",
        hidden: false,
    },
    Command {
        name: "compact",
        aliases: &[],
        description: "Compact conversation history to free context",
        hidden: false,
    },
    Command {
        name: "cost",
        aliases: &[],
        description: "Show session cost and token usage",
        hidden: false,
    },
    Command {
        name: "model",
        aliases: &[],
        description: "Show or change the current model",
        hidden: false,
    },
    Command {
        name: "diff",
        aliases: &[],
        description: "Show git diff of current changes",
        hidden: false,
    },
    Command {
        name: "status",
        aliases: &[],
        description: "Show git status",
        hidden: false,
    },
    Command {
        name: "commit",
        aliases: &[],
        description: "Ask the agent to commit current changes",
        hidden: false,
    },
    Command {
        name: "memory",
        aliases: &[],
        description: "Show loaded memory context",
        hidden: false,
    },
    Command {
        name: "skills",
        aliases: &[],
        description: "List available skills",
        hidden: false,
    },
    Command {
        name: "verbose",
        aliases: &[],
        description: "Toggle verbose output",
        hidden: false,
    },
    Command {
        name: "version",
        aliases: &[],
        description: "Show version information",
        hidden: true,
    },
];

/// Execute a slash command. Returns how to proceed.
pub fn execute(input: &str, engine: &mut QueryEngine) -> CommandResult {
    let input = input.trim_start_matches('/');
    let (cmd, args) = input
        .split_once(' ')
        .map(|(c, a)| (c, Some(a.trim())))
        .unwrap_or((input, None));

    // Check built-in commands.
    let matched = COMMANDS.iter().find(|c| {
        c.name == cmd || c.aliases.contains(&cmd)
    });

    match matched.map(|c| c.name) {
        Some("help") => {
            println!("\nAvailable commands:\n");
            for c in COMMANDS.iter().filter(|c| !c.hidden) {
                let aliases = if c.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", c.aliases.join(", "))
                };
                println!("  /{}{:<12} {}", c.name, aliases, c.description);
            }

            // Show skill commands.
            let skills = crate::skills::SkillRegistry::load_all(
                Some(std::path::Path::new(&engine.state().cwd)),
            );
            let invocable = skills.user_invocable();
            if !invocable.is_empty() {
                println!("\nSkills:");
                for skill in invocable {
                    let desc = skill
                        .metadata
                        .description
                        .as_deref()
                        .unwrap_or("");
                    println!("  /{:<18} {}", skill.name, desc);
                }
            }
            println!();
            CommandResult::Handled
        }
        Some("exit") => CommandResult::Exit,
        Some("clear") => {
            engine.state_mut().messages.clear();
            println!("Conversation cleared.");
            CommandResult::Handled
        }
        Some("compact") => {
            let freed = crate::services::compact::microcompact(
                &mut engine.state_mut().messages,
                2,
            );
            if freed > 0 {
                println!("Freed ~{freed} estimated tokens.");
            } else {
                println!("Nothing to compact.");
            }
            CommandResult::Handled
        }
        Some("cost") => {
            let state = engine.state();
            let usage = &state.total_usage;
            println!(
                "Turns: {}\nTokens: {} (in: {}, out: {}, cache_write: {}, cache_read: {})\nCost: ${:.4}",
                state.turn_count,
                usage.total(),
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_creation_input_tokens,
                usage.cache_read_input_tokens,
                state.total_cost_usd,
            );
            CommandResult::Handled
        }
        Some("model") => {
            if let Some(new_model) = args {
                engine.state_mut().config.api.model = new_model.to_string();
                println!("Model changed to: {new_model}");
            } else {
                println!("Model: {}", engine.state().config.api.model);
            }
            CommandResult::Handled
        }
        Some("diff") => {
            CommandResult::Prompt("Run `git diff` and show me the changes.".to_string())
        }
        Some("status") => {
            CommandResult::Prompt("Run `git status` and show me the result.".to_string())
        }
        Some("commit") => {
            let msg = if let Some(m) = args {
                format!("Commit the current changes with message: {m}")
            } else {
                "Review the current git diff and create an appropriate commit.".to_string()
            };
            CommandResult::Prompt(msg)
        }
        Some("memory") => {
            let memory = crate::memory::MemoryContext::load(
                Some(std::path::Path::new(&engine.state().cwd)),
            );
            if memory.is_empty() {
                println!("No memory loaded.");
            } else {
                if memory.project_context.is_some() {
                    println!("Project context: loaded");
                }
                if memory.user_memory.is_some() {
                    println!("User memory: loaded ({} files)", memory.memory_files.len());
                }
            }
            CommandResult::Handled
        }
        Some("skills") => {
            let skills = crate::skills::SkillRegistry::load_all(
                Some(std::path::Path::new(&engine.state().cwd)),
            );
            if skills.all().is_empty() {
                println!("No skills loaded. Add .md files to .rc/skills/ or ~/.config/rust-code/skills/");
            } else {
                println!("Loaded {} skills:", skills.all().len());
                for skill in skills.all() {
                    let invocable = if skill.metadata.user_invocable { " [invocable]" } else { "" };
                    let desc = skill.metadata.description.as_deref().unwrap_or("");
                    println!("  {}{} — {}", skill.name, invocable, desc);
                }
            }
            CommandResult::Handled
        }
        Some("verbose") => {
            println!("Verbose mode toggled.");
            CommandResult::Handled
        }
        Some("version") => {
            println!("rc {}", env!("CARGO_PKG_VERSION"));
            CommandResult::Handled
        }
        _ => {
            // Check if it's a skill invocation.
            let skills = crate::skills::SkillRegistry::load_all(
                Some(std::path::Path::new(&engine.state().cwd)),
            );
            if let Some(skill) = skills.find(cmd) {
                let expanded = skill.expand(args);
                CommandResult::Prompt(expanded)
            } else {
                // Unknown command — pass through as prompt.
                CommandResult::Passthrough(format!("/{input}"))
            }
        }
    }
}
