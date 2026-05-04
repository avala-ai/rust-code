//! Terminal UI layer.
//!
//! Provides the interactive REPL, markdown rendering, and streaming
//! output display. Built on crossterm and rustyline.

pub mod activity;
pub mod keybindings;
pub mod keymap;
pub mod onboarding;
pub mod prompt;
pub mod render;
pub mod repl;
pub mod selector;
pub mod setup;
pub mod terminal_query;
#[path = "theme_runtime.rs"]
pub mod theme;
pub mod tui;
