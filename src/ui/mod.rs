//! Terminal UI layer.
//!
//! Provides the interactive REPL, markdown rendering, and streaming
//! output display. Built on crossterm and rustyline.

pub mod activity;
pub mod keybindings;
pub mod keymap;
pub mod render;
pub mod repl;
