//! Terminal rendering for markdown and code.
//!
//! Converts markdown text to styled terminal output with syntax
//! highlighting for fenced code blocks. Uses termimad for markdown
//! structure and syntect for code coloring.

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

/// Render markdown text to the terminal with syntax highlighting.
///
/// Handles fenced code blocks (```lang ... ```) with syntect coloring,
/// and delegates the rest to termimad for structural rendering.
pub fn render_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();
    let mut table_buffer: Vec<String> = Vec::new();

    for line in text.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End of code block — render with syntax highlighting.
                output.push_str(&highlight_code(&code_buffer, &code_lang));
                output.push('\n');
                code_buffer.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                // Start of code block — extract language hint.
                code_lang = line.trim_start_matches('`').trim().to_string();
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            code_buffer.push_str(line);
            code_buffer.push('\n');
        } else if is_table_line(line) {
            // Accumulate table lines for batch rendering.
            table_buffer.push(line.to_string());
        } else {
            // Flush any accumulated table before non-table content.
            if !table_buffer.is_empty() {
                output.push_str(&render_table(&table_buffer));
                table_buffer.clear();
            }
            // Render inline markdown elements.
            output.push_str(&render_inline(line));
            output.push('\n');
        }
    }

    // Flush any trailing table.
    if !table_buffer.is_empty() {
        output.push_str(&render_table(&table_buffer));
    }

    // Handle unclosed code block.
    if in_code_block && !code_buffer.is_empty() {
        output.push_str(&highlight_code(&code_buffer, &code_lang));
        output.push('\n');
    }

    output
}

/// Syntax-highlight a code block using syntect.
fn highlight_code(code: &str, lang: &str) -> String {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    // Find syntax by language name or extension.
    let syntax = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = String::new();

    for line in LinesWithEndings::from(code) {
        match highlighter.highlight_line(line, &ss) {
            Ok(ranges) => {
                let escaped = as_24_bit_terminal_escaped(&ranges, false);
                result.push_str(&escaped);
            }
            Err(_) => {
                result.push_str(line);
            }
        }
    }

    // Reset terminal colors.
    result.push_str("\x1b[0m");
    result
}

/// Render inline markdown elements (bold, italic, code spans, links).
/// Uses the active theme for colors instead of hardcoded ANSI codes.
fn render_inline(line: &str) -> String {
    let t = super::theme::current();

    // Convert theme Color to ANSI escape string.
    let accent_code = color_to_ansi(t.accent);
    let tool_code = color_to_ansi(t.tool);

    let mut result = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Inline code: `code`
        if chars[i] == '`'
            && let Some(end) = find_closing(&chars, i + 1, '`')
        {
            let code: String = chars[i + 1..end].iter().collect();
            result.push_str(&format!("{accent_code}{code}\x1b[0m"));
            i = end + 1;
            continue;
        }

        // Bold: **text** or __text__
        if i + 1 < chars.len()
            && chars[i] == '*'
            && chars[i + 1] == '*'
            && let Some(end) = find_double_closing(&chars, i + 2, '*')
        {
            let text: String = chars[i + 2..end].iter().collect();
            result.push_str(&format!("\x1b[1m{text}\x1b[0m"));
            i = end + 2;
            continue;
        }

        // Italic: *text* or _text_
        if chars[i] == '*' || chars[i] == '_' {
            let marker = chars[i];
            if i + 1 < chars.len()
                && chars[i + 1] != ' '
                && let Some(end) = find_closing(&chars, i + 1, marker)
                && end > i + 1
            {
                let text: String = chars[i + 1..end].iter().collect();
                result.push_str(&format!("\x1b[3m{text}\x1b[0m"));
                i = end + 1;
                continue;
            }
        }

        // Headings: # at start of line.
        if i == 0 && chars[i] == '#' {
            let level = chars.iter().take_while(|&&c| c == '#').count();
            let text: String = chars[level..].iter().collect();
            let text = text.trim_start();
            result.push_str(&format!("\x1b[1;4m{text}\x1b[0m"));
            return result;
        }

        // List items: - or * at start.
        if i == 0 && (chars[i] == '-' || chars[i] == '*') && chars.get(1) == Some(&' ') {
            let text: String = chars[2..].iter().collect();
            result.push_str(&format!("  {tool_code}•\x1b[0m {text}"));
            return result;
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Convert a crossterm Color to an ANSI escape sequence prefix.
fn color_to_ansi(color: crossterm::style::Color) -> String {
    match color {
        crossterm::style::Color::Rgb { r, g, b } => format!("\x1b[38;2;{r};{g};{b}m"),
        crossterm::style::Color::DarkCyan => "\x1b[36m".to_string(),
        crossterm::style::Color::Cyan => "\x1b[96m".to_string(),
        crossterm::style::Color::Red => "\x1b[31m".to_string(),
        crossterm::style::Color::Green => "\x1b[32m".to_string(),
        crossterm::style::Color::Yellow => "\x1b[33m".to_string(),
        crossterm::style::Color::Blue => "\x1b[34m".to_string(),
        crossterm::style::Color::Magenta => "\x1b[35m".to_string(),
        crossterm::style::Color::Grey => "\x1b[37m".to_string(),
        crossterm::style::Color::DarkGrey => "\x1b[90m".to_string(),
        crossterm::style::Color::White => "\x1b[97m".to_string(),
        crossterm::style::Color::Black => "\x1b[30m".to_string(),
        _ => "\x1b[36m".to_string(), // fallback to cyan
    }
}

/// Detect whether a line looks like a markdown table row (`| col | col |`).
fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() > 1
}

/// Returns true if this table row is a separator line like `|---|---|`.
fn is_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed
        .trim_start_matches('|')
        .trim_end_matches('|')
        .chars()
        .all(|c| c == '-' || c == ':' || c == '|' || c == ' ')
}

/// Parse a table row into cells (splitting on `|` and trimming).
fn parse_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    // Strip leading and trailing pipes, then split on `|`.
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

/// Render accumulated markdown table lines using box-drawing characters.
fn render_table(lines: &[String]) -> String {
    // Parse all rows, skipping separator rows.
    let rows: Vec<Vec<String>> = lines
        .iter()
        .filter(|l| !is_separator_row(l))
        .map(|l| parse_table_row(l))
        .collect();

    if rows.is_empty() {
        return String::new();
    }

    // Determine the number of columns and max width per column.
    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths = vec![0usize; num_cols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }
    }
    // Ensure at least width 1 per column.
    for w in &mut col_widths {
        if *w == 0 {
            *w = 1;
        }
    }

    let mut out = String::new();

    // Top border: ┌───┬───┐
    out.push('┌');
    for (i, &w) in col_widths.iter().enumerate() {
        for _ in 0..w + 2 {
            out.push('─');
        }
        out.push(if i + 1 < num_cols { '┬' } else { '┐' });
    }
    out.push('\n');

    for (row_idx, row) in rows.iter().enumerate() {
        // Data row: │ val │ val │
        out.push('│');
        for (i, w) in col_widths.iter().enumerate() {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            out.push(' ');
            out.push_str(cell);
            for _ in 0..*w - cell.len() {
                out.push(' ');
            }
            out.push_str(" │");
        }
        out.push('\n');

        // After the header row (row 0), draw a separator: ├───┼───┤
        if row_idx == 0 && rows.len() > 1 {
            out.push('├');
            for (i, &w) in col_widths.iter().enumerate() {
                for _ in 0..w + 2 {
                    out.push('─');
                }
                out.push(if i + 1 < num_cols { '┼' } else { '┤' });
            }
            out.push('\n');
        }
    }

    // Bottom border: └───┴───┘
    out.push('└');
    for (i, &w) in col_widths.iter().enumerate() {
        for _ in 0..w + 2 {
            out.push('─');
        }
        out.push(if i + 1 < num_cols { '┴' } else { '┘' });
    }
    out.push('\n');

    out
}

fn find_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    (start..chars.len()).find(|&i| chars[i] == marker)
}

fn find_double_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    (start..chars.len().saturating_sub(1)).find(|&i| chars[i] == marker && chars[i + 1] == marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_heading() {
        let result = render_inline("# Hello World");
        assert!(result.contains("Hello World"));
        assert!(result.contains("\x1b[1;4m")); // bold + underline
    }

    #[test]
    fn test_render_list_item() {
        let result = render_inline("- item one");
        assert!(result.contains("•"));
        assert!(result.contains("item one"));
    }

    #[test]
    fn test_highlight_code_doesnt_panic() {
        let result = highlight_code("fn main() {}\n", "rust");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_is_table_line() {
        assert!(is_table_line("| a | b |"));
        assert!(is_table_line("| --- | --- |"));
        assert!(!is_table_line("not a table"));
        assert!(!is_table_line("|"));
        assert!(!is_table_line("| no closing pipe"));
    }

    #[test]
    fn test_render_table_basic() {
        let lines = vec![
            "| Name | Age |".to_string(),
            "| --- | --- |".to_string(),
            "| Alice | 30 |".to_string(),
            "| Bob | 25 |".to_string(),
        ];
        let result = render_table(&lines);
        assert!(result.contains('┌'));
        assert!(result.contains('┘'));
        assert!(result.contains("Alice"));
        assert!(result.contains("Bob"));
        // Header separator present.
        assert!(result.contains('┼'));
    }

    #[test]
    fn test_render_markdown_with_table() {
        let md = "# Title\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\nDone.";
        let result = render_markdown(md);
        assert!(result.contains("Title"));
        assert!(result.contains('┌'));
        assert!(result.contains("Done."));
    }
}
