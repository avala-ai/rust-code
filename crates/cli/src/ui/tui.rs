//! Ratatui-based TUI rendering.
//!
//! Provides rich terminal UI for tool execution, status display,
//! and formatted output. Operates inline (no alternate screen) to
//! coexist with rustyline for input.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---- Theme bridge ----

/// Convert crossterm Color to ratatui Color.
pub fn theme_to_ratatui(color: crossterm::style::Color) -> Color {
    match color {
        crossterm::style::Color::Rgb { r, g, b } => Color::Rgb(r, g, b),
        crossterm::style::Color::Black => Color::Black,
        crossterm::style::Color::Red => Color::Red,
        crossterm::style::Color::Green => Color::Green,
        crossterm::style::Color::Yellow => Color::Yellow,
        crossterm::style::Color::Blue => Color::Blue,
        crossterm::style::Color::Magenta => Color::Magenta,
        crossterm::style::Color::Cyan => Color::Cyan,
        crossterm::style::Color::White => Color::White,
        crossterm::style::Color::DarkGrey => Color::DarkGray,
        crossterm::style::Color::Grey => Color::Gray,
        crossterm::style::Color::DarkCyan => Color::Cyan,
        crossterm::style::Color::DarkGreen => Color::Green,
        crossterm::style::Color::DarkYellow => Color::Yellow,
        crossterm::style::Color::DarkMagenta => Color::Magenta,
        crossterm::style::Color::DarkRed => Color::Red,
        crossterm::style::Color::DarkBlue => Color::Blue,
        _ => Color::Reset,
    }
}

// ---- Turn state tracking ----

/// Tracks tool executions during a single agent turn for summary display.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub name: String,
    pub detail: String,
    pub result_preview: Option<String>,
    pub is_error: bool,
    pub line_count: usize,
}

/// Accumulated state for the current turn's TUI display.
#[derive(Debug, Default, Clone)]
pub struct TurnState {
    pub tools: Vec<ToolEntry>,
    pub thinking_chars: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TurnState {
    pub fn clear(&mut self) {
        self.tools.clear();
        self.thinking_chars = 0;
        self.tokens_in = 0;
        self.tokens_out = 0;
        self.cache_read = 0;
        self.cache_write = 0;
    }

    pub fn add_tool_start(&mut self, name: &str, detail: &str) {
        self.tools.push(ToolEntry {
            name: name.to_string(),
            detail: detail.to_string(),
            result_preview: None,
            is_error: false,
            line_count: 0,
        });
    }

    pub fn complete_last_tool(&mut self, result: &str, is_error: bool) {
        if let Some(tool) = self.tools.last_mut() {
            let preview = result.lines().next().unwrap_or("(ok)");
            tool.result_preview = Some(truncate(preview, 80));
            tool.is_error = is_error;
            tool.line_count = result.lines().count();
        }
    }
}

/// Shared turn state for the TUI sink.
pub type SharedTurnState = Arc<Mutex<TurnState>>;

pub fn new_turn_state() -> SharedTurnState {
    Arc::new(Mutex::new(TurnState::default()))
}

// ---- Rendering functions ----

/// Render a tool execution header inline.
pub fn render_tool_block(tool_name: &str, detail: &str, _result: Option<&str>, _is_error: bool) {
    let t = super::theme::current();
    let accent = theme_to_ratatui(t.tool);
    let muted = theme_to_ratatui(t.muted);

    let width = term_width();

    let lines = vec![Line::from(vec![
        Span::styled(
            format!(" {tool_name} "),
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            truncate(detail, width.saturating_sub(tool_name.len() + 6)),
            Style::default().fg(muted),
        ),
    ])];

    let buf = render_lines_to_ansi(&lines);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

/// Render the thinking indicator.
pub fn render_thinking_block(text: &str) {
    let t = super::theme::current();
    let muted = theme_to_ratatui(t.muted);
    let width = term_width().saturating_sub(8);

    let preview = if text.len() <= width {
        text.trim().to_string()
    } else {
        let p: String = text.chars().take(width.saturating_sub(20)).collect();
        format!("{p}... ({} chars)", text.len())
    };

    let line = Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            preview,
            Style::default().fg(muted).add_modifier(Modifier::ITALIC),
        ),
    ]);

    let buf = render_lines_to_ansi(&[line]);
    eprint!("\r{buf}\r");
    let _ = io::stderr().flush();
}

/// Render a turn summary panel showing all tool executions.
pub fn render_turn_summary(state: &TurnState, turn: usize) {
    if state.tools.is_empty() {
        return;
    }

    let t = super::theme::current();
    let accent = theme_to_ratatui(t.accent);
    let muted = theme_to_ratatui(t.muted);
    let success = theme_to_ratatui(t.success);
    let error = theme_to_ratatui(t.error);

    let width = term_width();
    let mut lines = Vec::new();

    // Top border.
    let border = "─".repeat(width.saturating_sub(4));
    lines.push(Line::from(vec![
        Span::styled("  ╭", Style::default().fg(muted)),
        Span::styled(border.clone(), Style::default().fg(muted)),
    ]));

    // Header.
    let tool_count = state.tools.len();
    let pass_count = state.tools.iter().filter(|t| !t.is_error).count();
    let fail_count = tool_count - pass_count;
    let header = format!(
        " turn {turn}: {tool_count} tool{} ({pass_count} ok{})",
        if tool_count != 1 { "s" } else { "" },
        if fail_count > 0 {
            format!(", {fail_count} err")
        } else {
            String::new()
        },
    );
    lines.push(Line::from(vec![
        Span::styled("  │ ", Style::default().fg(muted)),
        Span::styled(
            header,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Separator.
    lines.push(Line::from(vec![
        Span::styled("  ├", Style::default().fg(muted)),
        Span::styled(
            "─".repeat(width.saturating_sub(4)),
            Style::default().fg(muted),
        ),
    ]));

    // Tool list.
    for tool in &state.tools {
        let (icon, color) = if tool.is_error {
            ("✗", error)
        } else {
            ("✓", success)
        };

        let result_info = if let Some(ref preview) = tool.result_preview {
            let suffix = if tool.line_count > 1 {
                format!(" (+{})", tool.line_count - 1)
            } else {
                String::new()
            };
            format!(" → {}{}", truncate(preview, 50), suffix)
        } else {
            String::new()
        };

        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(muted)),
            Span::styled(format!("{icon} "), Style::default().fg(color)),
            Span::styled(
                &tool.name,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}", truncate(&tool.detail, 40)),
                Style::default().fg(muted),
            ),
            Span::styled(result_info, Style::default().fg(muted)),
        ]));
    }

    // Token usage.
    if state.tokens_in > 0 || state.tokens_out > 0 {
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(muted)),
            Span::styled(
                format!(
                    "⟡ {}in · {}out{}{}",
                    state.tokens_in,
                    state.tokens_out,
                    if state.cache_read > 0 {
                        format!(" · {}cache↓", state.cache_read)
                    } else {
                        String::new()
                    },
                    if state.cache_write > 0 {
                        format!(" · {}cache↑", state.cache_write)
                    } else {
                        String::new()
                    },
                ),
                Style::default().fg(muted),
            ),
        ]));
    }

    // Bottom border.
    lines.push(Line::from(vec![
        Span::styled("  ╰", Style::default().fg(muted)),
        Span::styled(border, Style::default().fg(muted)),
    ]));

    let buf = render_lines_to_ansi(&lines);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

/// Render the pending warnings as a bannered block on stderr.
///
/// No-op when the registry is empty, so this is safe to call on every
/// REPL cycle. Styling: each line is prefixed with a `[WARN]` /
/// `[INFO]` tag in yellow / muted cyan respectively. Written to
/// stderr so the banner doesn't get piped into downstream tools when
/// stdout is captured.
pub fn render_warnings_banner() {
    let warnings = agent_code_lib::services::warnings::snapshot();
    if warnings.is_empty() {
        return;
    }

    let t = super::theme::current();
    let warn_tag = theme_to_ratatui(t.warning);
    let info_tag = theme_to_ratatui(t.muted);
    let border = theme_to_ratatui(t.muted);

    let width = term_width();
    let hr = "─".repeat(width.min(78));

    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        hr.clone(),
        Style::default().fg(border),
    )]));
    for w in &warnings {
        use agent_code_lib::services::warnings::WarningLevel;
        let (tag_color, tag_label) = match w.level {
            WarningLevel::Warn => (warn_tag, "[WARN]"),
            WarningLevel::Info => (info_tag, "[INFO]"),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {tag_label} "),
                Style::default().fg(tag_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::raw(w.message.clone()),
        ]));
    }
    lines.push(Line::from(vec![Span::styled(
        hr,
        Style::default().fg(border),
    )]));

    let buf = render_lines_to_ansi(&lines);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

/// Render a status bar at the bottom of output.
pub fn render_status_bar(model: &str, turn: usize, tokens: u64, cost: f64) {
    let t = super::theme::current();
    let accent = theme_to_ratatui(t.accent);
    let muted = theme_to_ratatui(t.muted);

    let width = term_width();
    let left = format!(" {model} ");
    let right = format!(" turn {} │ {} tokens │ ${:.4} ", turn, tokens, cost);
    let padding = width.saturating_sub(left.len() + right.len());

    let line = Line::from(vec![
        Span::styled(
            left,
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding), Style::default()),
        Span::styled(right, Style::default().fg(muted)),
    ]);

    let buf = render_lines_to_ansi(&[line]);
    eprint!("{buf}");
    let _ = io::stderr().flush();
}

// ---- Internal helpers ----

fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .min(120)
}

/// Render ratatui Lines to ANSI escape string (inline, no alternate screen).
fn render_lines_to_ansi(lines: &[Line<'_>]) -> String {
    let mut buf = String::new();
    for line in lines {
        for span in &line.spans {
            let mut codes = Vec::new();
            if let Some(fg) = span.style.fg {
                match fg {
                    Color::Rgb(r, g, b) => codes.push(format!("38;2;{r};{g};{b}")),
                    _ => codes.push(color_to_fg_code(fg).to_string()),
                }
            }
            if let Some(bg) = span.style.bg {
                match bg {
                    Color::Rgb(r, g, b) => codes.push(format!("48;2;{r};{g};{b}")),
                    _ => codes.push(color_to_bg_code(bg).to_string()),
                }
            }
            if span.style.add_modifier.contains(Modifier::BOLD) {
                codes.push("1".to_string());
            }
            if span.style.add_modifier.contains(Modifier::ITALIC) {
                codes.push("3".to_string());
            }
            if span.style.add_modifier.contains(Modifier::UNDERLINED) {
                codes.push("4".to_string());
            }

            if !codes.is_empty() {
                buf.push_str(&format!("\x1b[{}m", codes.join(";")));
            }
            buf.push_str(&span.content);
            if !codes.is_empty() {
                buf.push_str("\x1b[0m");
            }
        }
        buf.push_str("\r\n");
    }
    buf
}

fn color_to_fg_code(color: Color) -> String {
    match color {
        Color::Black => "30".into(),
        Color::Red => "31".into(),
        Color::Green => "32".into(),
        Color::Yellow => "33".into(),
        Color::Blue => "34".into(),
        Color::Magenta => "35".into(),
        Color::Cyan => "36".into(),
        Color::White | Color::Gray => "37".into(),
        Color::DarkGray => "90".into(),
        Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        _ => "39".into(),
    }
}

fn color_to_bg_code(color: Color) -> String {
    match color {
        Color::Black => "40".into(),
        Color::Red => "41".into(),
        Color::Green => "42".into(),
        Color::Yellow => "43".into(),
        Color::Blue => "44".into(),
        Color::Magenta => "45".into(),
        Color::Cyan => "46".into(),
        Color::White | Color::Gray => "47".into(),
        Color::DarkGray => "100".into(),
        Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
        _ => "49".into(),
    }
}

// ---- Scrollback viewer ----

/// Interactive scrollable conversation history viewer.
/// Uses crossterm raw mode for keyboard input. Press q or Esc to exit.
pub fn scrollback_viewer(messages: &[agent_code_lib::llm::message::Message]) {
    use crossterm::{
        event::{self, Event, KeyCode, KeyEvent},
        terminal,
    };

    let t = super::theme::current();
    let accent = theme_to_ratatui(t.accent);
    let muted = theme_to_ratatui(t.muted);
    let success = theme_to_ratatui(t.success);
    let error = theme_to_ratatui(t.error);

    // Build display lines from messages.
    let mut all_lines: Vec<Line<'static>> = Vec::new();

    for (idx, msg) in messages.iter().enumerate() {
        match msg {
            agent_code_lib::llm::message::Message::User(u) => {
                // User header.
                all_lines.push(Line::from(vec![Span::styled(
                    format!(" [{idx}] USER "),
                    Style::default()
                        .fg(Color::Black)
                        .bg(accent)
                        .add_modifier(Modifier::BOLD),
                )]));
                // User content.
                for block in &u.content {
                    match block {
                        agent_code_lib::llm::message::ContentBlock::Text { text } => {
                            for line in text.lines() {
                                all_lines.push(Line::from(Span::raw(format!("  {line}"))));
                            }
                        }
                        agent_code_lib::llm::message::ContentBlock::ToolResult {
                            content,
                            is_error,
                            ..
                        } => {
                            let color = if *is_error { error } else { success };
                            let icon = if *is_error { "✗" } else { "✓" };
                            let preview: String = content
                                .lines()
                                .next()
                                .unwrap_or("")
                                .chars()
                                .take(60)
                                .collect();
                            all_lines.push(Line::from(vec![
                                Span::styled(format!("  {icon} "), Style::default().fg(color)),
                                Span::styled(preview, Style::default().fg(muted)),
                            ]));
                        }
                        _ => {}
                    }
                }
                all_lines.push(Line::from(""));
            }
            agent_code_lib::llm::message::Message::Assistant(a) => {
                // Assistant header.
                let model_tag = a
                    .model
                    .as_deref()
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default();
                all_lines.push(Line::from(vec![Span::styled(
                    format!(" [{idx}] ASSISTANT{model_tag} "),
                    Style::default()
                        .fg(Color::Black)
                        .bg(success)
                        .add_modifier(Modifier::BOLD),
                )]));
                // Content.
                let mut tool_count = 0;
                for block in &a.content {
                    match block {
                        agent_code_lib::llm::message::ContentBlock::Text { text } => {
                            for line in text.lines().take(20) {
                                all_lines.push(Line::from(Span::raw(format!("  {line}"))));
                            }
                            if text.lines().count() > 20 {
                                all_lines.push(Line::from(Span::styled(
                                    format!("  ... ({} more lines)", text.lines().count() - 20),
                                    Style::default().fg(muted),
                                )));
                            }
                        }
                        agent_code_lib::llm::message::ContentBlock::ToolUse { name, .. } => {
                            tool_count += 1;
                            all_lines.push(Line::from(vec![
                                Span::styled("  → ", Style::default().fg(muted)),
                                Span::styled(
                                    name.to_string(),
                                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                                ),
                            ]));
                        }
                        agent_code_lib::llm::message::ContentBlock::Thinking {
                            thinking, ..
                        } => {
                            let preview: String = thinking.chars().take(80).collect();
                            all_lines.push(Line::from(Span::styled(
                                format!("  (thinking: {preview}...)"),
                                Style::default().fg(muted).add_modifier(Modifier::ITALIC),
                            )));
                        }
                        _ => {}
                    }
                }
                if tool_count > 0 {
                    all_lines.push(Line::from(Span::styled(
                        format!(
                            "  ({tool_count} tool call{})",
                            if tool_count != 1 { "s" } else { "" }
                        ),
                        Style::default().fg(muted),
                    )));
                }
                all_lines.push(Line::from(""));
            }
            _ => {}
        }
    }

    if all_lines.is_empty() {
        return;
    }

    // Enter scrollable view.
    let (term_w, term_h) = terminal::size().unwrap_or((80, 24));
    let view_height = (term_h as usize).saturating_sub(3); // Reserve for header + footer.
    let max_scroll = all_lines.len().saturating_sub(view_height);
    let mut scroll: usize = max_scroll; // Start at bottom (most recent).

    // Search state.
    let mut search_query: Option<String> = None;
    let mut matches: Vec<usize> = Vec::new();
    let mut match_idx: usize = 0;

    terminal::enable_raw_mode().expect("raw mode");

    render_scrollback(
        &all_lines,
        scroll,
        view_height,
        term_w as usize,
        messages.len(),
        search_query.as_deref(),
        &matches,
        match_idx,
    );

    loop {
        if let Ok(Event::Key(KeyEvent { code, .. })) = event::read() {
            match code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up | KeyCode::Char('k') => {
                    scroll = scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    scroll = scroll.min(max_scroll).saturating_add(1).min(max_scroll);
                }
                KeyCode::PageUp => {
                    scroll = scroll.saturating_sub(view_height);
                }
                KeyCode::PageDown => {
                    scroll = (scroll + view_height).min(max_scroll);
                }
                KeyCode::Home => {
                    scroll = 0;
                }
                KeyCode::End => {
                    scroll = max_scroll;
                }
                KeyCode::Char('/') => {
                    // Enter search-input submode. Returns Some(query) on
                    // Enter, None on Esc/empty.
                    if let Some(query) = read_search_query(view_height, term_w as usize) {
                        matches = find_matches(&all_lines, &query);
                        search_query = Some(query);
                        match_idx = 0;
                        if let Some(&first) = matches.first() {
                            scroll = first.saturating_sub(view_height / 3).min(max_scroll);
                        }
                    }
                }
                KeyCode::Char('n') => {
                    if !matches.is_empty() {
                        match_idx = (match_idx + 1) % matches.len();
                        scroll = matches[match_idx]
                            .saturating_sub(view_height / 3)
                            .min(max_scroll);
                    }
                }
                KeyCode::Char('N') => {
                    if !matches.is_empty() {
                        match_idx = if match_idx == 0 {
                            matches.len() - 1
                        } else {
                            match_idx - 1
                        };
                        scroll = matches[match_idx]
                            .saturating_sub(view_height / 3)
                            .min(max_scroll);
                    }
                }
                _ => continue,
            }
            // Clear and re-render.
            clear_scrollback(view_height + 2);
            render_scrollback(
                &all_lines,
                scroll,
                view_height,
                term_w as usize,
                messages.len(),
                search_query.as_deref(),
                &matches,
                match_idx,
            );
        }
    }

    terminal::disable_raw_mode().expect("disable raw mode");
    clear_scrollback(view_height + 2);
    println!("  (exited scroll view)\r");
}

/// Read a `/search` query from the user in raw mode. Returns
/// `Some(query)` on Enter, `None` on Esc or empty input. Renders an
/// inline prompt at the bottom of the terminal.
fn read_search_query(view_height: usize, width: usize) -> Option<String> {
    use crossterm::{
        event::{self, Event, KeyCode, KeyEvent},
        terminal,
    };

    let mut buf = String::new();
    let t = super::theme::current();
    let accent = theme_to_ratatui(t.accent);

    loop {
        // Render just the footer with the prompt — overwrite the footer
        // line in place (up one row, clear, print).
        let prompt = format!(" /{buf}█ ");
        let line = Line::from(Span::styled(
            truncate(&prompt, width),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
        let stdout = io::stdout();
        let mut out = stdout.lock();
        // Move to bottom of viewport and overwrite the footer line.
        write!(out, "\x1b[{};1H\x1b[2K", view_height + 2).ok();
        write!(out, "{}", render_lines_to_ansi(&[line])).ok();
        out.flush().ok();

        let ev = match event::read() {
            Ok(Event::Key(KeyEvent { code, .. })) => code,
            _ => continue,
        };

        match ev {
            KeyCode::Esc => return None,
            KeyCode::Enter => {
                // Re-enable raw mode in case child loop disabled it.
                let _ = terminal::enable_raw_mode();
                return if buf.is_empty() { None } else { Some(buf) };
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(c) => {
                buf.push(c);
            }
            _ => continue,
        }
    }
}

/// Find every line index whose text content contains `query` (case-
/// insensitive). Matched against the concatenated span text of each
/// line, stripping ANSI.
fn find_matches(lines: &[Line<'_>], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            if text.to_lowercase().contains(&q) {
                Some(i)
            } else {
                None
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_scrollback(
    lines: &[Line<'_>],
    scroll: usize,
    view_height: usize,
    width: usize,
    msg_count: usize,
    search_query: Option<&str>,
    matches: &[usize],
    match_idx: usize,
) {
    let t = super::theme::current();
    let muted = theme_to_ratatui(t.muted);
    let accent = theme_to_ratatui(t.accent);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Header: include match indicator when searching.
    let header = match search_query {
        Some(q) if !matches.is_empty() => format!(
            " Conversation ({msg_count} msgs) │ /{q} {}/{} │ n next · N prev · q quit ",
            match_idx + 1,
            matches.len(),
        ),
        Some(q) => {
            format!(" Conversation ({msg_count} msgs) │ /{q} (no matches) │ / search · q quit ")
        }
        None => format!(
            " Conversation ({msg_count} msgs) │ ↑↓/jk scroll │ PgUp/PgDn │ / search │ q quit "
        ),
    };
    let hdr_line = Line::from(Span::styled(
        truncate(&header, width),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    ));
    write!(out, "{}", render_lines_to_ansi(&[hdr_line])).ok();

    // Content.
    let end = (scroll + view_height).min(lines.len());
    let visible = &lines[scroll..end];
    let buf = render_lines_to_ansi(visible);
    write!(out, "{buf}").ok();

    // Pad remaining lines.
    for _ in visible.len()..view_height {
        write!(out, "  ~\r\n").ok();
    }

    // Footer: scroll position.
    let pct = if lines.len() <= view_height {
        100
    } else {
        (scroll * 100) / lines.len().saturating_sub(view_height).max(1)
    };
    let footer = format!(" line {}-{} of {} ({pct}%) ", scroll + 1, end, lines.len());
    let ftr_line = Line::from(Span::styled(
        truncate(&footer, width),
        Style::default().fg(muted),
    ));
    write!(out, "{}", render_lines_to_ansi(&[ftr_line])).ok();

    out.flush().ok();
}

fn clear_scrollback(lines: usize) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for _ in 0..lines {
        write!(out, "\x1b[A\x1b[2K").ok();
    }
    out.flush().ok();
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 3 {
        format!("{}...", &s[..max - 3])
    } else {
        s[..max].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_matches_case_insensitive_substring() {
        let lines = vec![
            Line::from(Span::raw("Hello World".to_string())),
            Line::from(Span::raw("foo bar".to_string())),
            Line::from(Span::raw("HELLO there".to_string())),
        ];
        let matches = find_matches(&lines, "hello");
        assert_eq!(matches, vec![0, 2]);
    }

    #[test]
    fn find_matches_empty_query_returns_empty() {
        let lines = vec![Line::from(Span::raw("Hello World".to_string()))];
        assert!(find_matches(&lines, "").is_empty());
    }

    #[test]
    fn find_matches_no_hit_returns_empty() {
        let lines = vec![Line::from(Span::raw("Hello World".to_string()))];
        assert!(find_matches(&lines, "xyz").is_empty());
    }

    #[test]
    fn find_matches_across_spans() {
        // The text is split into multiple spans — search must consider
        // the concatenation, not individual spans.
        let lines = vec![Line::from(vec![
            Span::raw("Hello ".to_string()),
            Span::styled("wor".to_string(), Style::default()),
            Span::raw("ld".to_string()),
        ])];
        let matches = find_matches(&lines, "hello world");
        assert_eq!(matches, vec![0]);
    }
}
