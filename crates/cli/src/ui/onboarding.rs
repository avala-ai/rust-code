//! First-run onboarding flow.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use agent_code_lib::config::{agent_config_dir, atomic::atomic_write_secret};
use crossterm::style::Stylize;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal,
};

use super::theme::Theme;

const SENTINEL_NAME: &str = ".onboarding-complete";
pub const DEFAULT_THEME: &str = "auto";

fn config_dir() -> Option<PathBuf> {
    agent_config_dir()
}

pub fn sentinel_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(SENTINEL_NAME))
}

pub fn already_onboarded() -> bool {
    sentinel_path().map(|p| p.exists()).unwrap_or(true)
}

pub fn mark_onboarded() {
    if let Some(path) = sentinel_path()
        && let Some(parent) = path.parent()
    {
        if !parent.exists() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = atomic_write_secret(&path, b"") {
            tracing::debug!("onboarding sentinel write failed: {e}");
        }
    }
}

#[derive(Debug, Clone)]
pub struct OnboardingResult {
    pub theme: String,
    pub interactive: bool,
}

pub fn run_first_run() -> OnboardingResult {
    if !is_interactive() {
        mark_onboarded();
        return OnboardingResult {
            theme: DEFAULT_THEME.to_string(),
            interactive: false,
        };
    }

    print_welcome();
    let chosen = pick_theme(default_theme_for_picker()).unwrap_or_else(|| DEFAULT_THEME.to_string());

    if let Err(e) = persist_theme(&chosen) {
        tracing::warn!("could not persist theme '{chosen}': {e}");
    }
    mark_onboarded();

    OnboardingResult {
        theme: chosen,
        interactive: true,
    }
}

pub fn rerun_theme_picker(current: &str) -> Option<String> {
    if !is_interactive() {
        return None;
    }
    let chosen = pick_theme(current.to_string())?;
    if let Err(e) = persist_theme(&chosen) {
        tracing::warn!("could not persist theme '{chosen}': {e}");
        return None;
    }
    Some(chosen)
}

fn default_theme_for_picker() -> String {
    "auto".to_string()
}

fn is_interactive() -> bool {
    io::stdout().is_terminal() && io::stdin().is_terminal()
}

const LOGO: &[&str] = &[
    "      +-------------------------------------+",
    "      |      AAAAA     CCCCC                |",
    "      |     AA   AA   CC                    |",
    "      |    AAAAAAA   CC                     |",
    "      |   AA     AA   CC                    |",
    "      |  AA       AA   CCCCC                |",
    "      +-------------------------------------+",
];

fn print_welcome() {
    let version = env!("CARGO_PKG_VERSION");
    let theme = super::theme::current();
    println!();
    println!(
        "  {}{}",
        "Welcome to agent-code v".with(theme.text),
        version.with(theme.accent),
    );
    println!();
    for line in LOGO {
        println!("  {}", line.with(theme.accent));
    }
    println!();
    println!("  {}", "Let's get started.".with(theme.text));
    println!();
}

struct PickerOption {
    label: &'static str,
    value: &'static str,
}

const THEME_OPTIONS: &[PickerOption] = &[
    PickerOption {
        label: "Auto (match terminal)",
        value: "auto",
    },
    PickerOption {
        label: "Dark mode",
        value: "midnight",
    },
    PickerOption {
        label: "Light mode",
        value: "daybreak",
    },
    PickerOption {
        label: "Dark mode (colorblind-friendly)",
        value: "dark-colorblind",
    },
    PickerOption {
        label: "Light mode (colorblind-friendly)",
        value: "light-colorblind",
    },
    PickerOption {
        label: "Dark mode (ANSI colors only)",
        value: "dark-ansi",
    },
    PickerOption {
        label: "Light mode (ANSI colors only)",
        value: "light-ansi",
    },
];

fn pick_theme(initial: String) -> Option<String> {
    let mut selected = THEME_OPTIONS
        .iter()
        .position(|o| o.value == initial)
        .unwrap_or(0);
    let confirmed_initial = selected;

    if terminal::enable_raw_mode().is_err() {
        return Some(THEME_OPTIONS[selected].value.to_string());
    }

    let stdout = io::stdout();
    {
        let mut out = stdout.lock();
        writeln!(
            out,
            "  Choose the text style that looks best with your terminal\r"
        )
        .ok();
        writeln!(out, "  To change this later, run /theme\r").ok();
        writeln!(out, "\r").ok();
        out.flush().ok();
    }

    render_picker(selected, confirmed_initial);

    let mut cancelled = false;
    loop {
        let event = match event::read() {
            Ok(e) => e,
            Err(_) => {
                cancelled = true;
                break;
            }
        };
        if let Event::Key(KeyEvent {
            code,
            kind,
            modifiers,
            ..
        }) = event
        {
            if kind != KeyEventKind::Press {
                continue;
            }
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = if selected == 0 {
                        THEME_OPTIONS.len() - 1
                    } else {
                        selected - 1
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1) % THEME_OPTIONS.len();
                }
                KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                    let idx = (c as usize) - ('1' as usize);
                    if idx < THEME_OPTIONS.len() {
                        selected = idx;
                    }
                }
                KeyCode::Enter => break,
                KeyCode::Esc => {
                    cancelled = true;
                    break;
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    cancelled = true;
                    break;
                }
                _ => {}
            }
            clear_picker();
            render_picker(selected, confirmed_initial);
        }
    }

    let _ = terminal::disable_raw_mode();
    clear_picker();

    if cancelled {
        println!(
            "  {} {}",
            ">".with(super::theme::current().muted),
            "Skipped - using auto".with(super::theme::current().muted),
        );
        return None;
    }

    let chosen = THEME_OPTIONS[selected].value.to_string();
    println!(
        "  {} {}",
        ">".with(super::theme::current().accent),
        chosen.clone().with(super::theme::current().text).bold(),
    );
    Some(chosen)
}

fn picker_height() -> usize {
    THEME_OPTIONS.len() + 1 + 5 + 1
}

fn clear_picker() {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for _ in 0..picker_height() {
        let _ = write!(out, "\x1b[A\x1b[2K");
    }
    out.flush().ok();
}

fn render_picker(selected: usize, confirmed_initial: usize) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let theme = Theme::from_name(THEME_OPTIONS[selected].value);

    for (i, opt) in THEME_OPTIONS.iter().enumerate() {
        let is_cursor = i == selected;
        let is_initial = i == confirmed_initial;
        let cursor_marker = if is_cursor { ">" } else { " " };
        let check = if is_initial { " *" } else { "" };
        let number = format!("{}.", i + 1);
        if is_cursor {
            let _ = writeln!(
                out,
                "   {} {} {}{}\r",
                cursor_marker.with(theme.accent).bold(),
                number.with(theme.accent),
                opt.label.with(theme.text).bold(),
                check.with(theme.success),
            );
        } else {
            let _ = writeln!(
                out,
                "   {} {} {}{}\r",
                cursor_marker,
                number,
                opt.label.with(theme.muted),
                check.with(theme.success),
            );
        }
    }

    let _ = writeln!(out, "\r");
    for line in render_preview(&theme).lines() {
        let _ = writeln!(out, "  {line}\r");
    }
    let _ = writeln!(
        out,
        "  {}\r",
        "Up/Down move | 1-7 jump | Enter confirm | Esc skip".with(theme.muted),
    );
    out.flush().ok();
}

fn render_preview(theme: &Theme) -> String {
    use std::fmt::Write as _;

    let rows = [
        (" ", "fn render(view: &View) {", theme.text),
        ("-", "    let title = view.heading();", theme.diff_remove),
        ("+", "    let title = view.heading_styled();", theme.diff_add),
        ("+", "    writeln!(view.out, \"{title}\")?;", theme.diff_add),
        (" ", "}", theme.text),
    ];

    let mut out = String::new();
    for (idx, (prefix, body, color)) in rows.into_iter().enumerate() {
        let text = if prefix == " " {
            body.to_string()
        } else {
            format!("{prefix} {body}")
        };
        let _ = writeln!(
            out,
            "{:>3}  {}",
            (idx + 1).to_string().with(theme.muted),
            text.with(color),
        );
    }
    out
}

fn persist_theme(theme_name: &str) -> Result<(), String> {
    let dir = config_dir().ok_or_else(|| "no user config directory".to_string())?;
    let path = dir.join("config.toml");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {dir:?}: {e}"))?;
    }

    let mut doc: toml::Value = if path.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        toml::from_str(&raw).map_err(|e| format!("parse {path:?}: {e}"))?
    } else {
        toml::Value::Table(toml::value::Table::new())
    };

    let table = doc
        .as_table_mut()
        .ok_or_else(|| "config.toml is not a table".to_string())?;
    let ui = table
        .entry("ui")
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let ui_table = ui
        .as_table_mut()
        .ok_or_else(|| "[ui] section is not a table".to_string())?;
    ui_table.insert(
        "theme".to_string(),
        toml::Value::String(theme_name.to_string()),
    );

    let serialized = toml::to_string_pretty(&doc).map_err(|e| format!("serialize: {e}"))?;
    atomic_write_secret(&path, serialized.as_bytes())
        .map_err(|e| format!("atomic write {path:?}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConfigSandbox {
        _tmp: tempfile::TempDir,
        prev_xdg: Option<String>,
        prev_home: Option<String>,
    }

    impl ConfigSandbox {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
            let prev_home = std::env::var("HOME").ok();
            unsafe {
                std::env::set_var("XDG_CONFIG_HOME", tmp.path());
                std::env::set_var("HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                prev_xdg,
                prev_home,
            }
        }
    }

    impl Drop for ConfigSandbox {
        fn drop(&mut self) {
            unsafe {
                match &self.prev_xdg {
                    Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                    None => std::env::remove_var("XDG_CONFIG_HOME"),
                }
                match &self.prev_home {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn sentinel_absent_then_present_after_mark() {
        let _g = ENV_LOCK.lock().unwrap();
        let _sandbox = ConfigSandbox::new();
        assert!(
            !already_onboarded(),
            "fresh sandbox should not be onboarded"
        );
        mark_onboarded();
        assert!(already_onboarded(), "mark_onboarded must drop sentinel");
        assert!(sentinel_path().unwrap().exists());
    }

    #[test]
    fn mark_onboarded_creates_parent_dir() {
        let _g = ENV_LOCK.lock().unwrap();
        let _sandbox = ConfigSandbox::new();
        let dir = config_dir().unwrap();
        assert!(!dir.exists());
        mark_onboarded();
        assert!(dir.exists());
        assert!(sentinel_path().unwrap().exists());
    }

    #[test]
    fn persist_theme_writes_ui_table_and_preserves_other_keys() {
        let _g = ENV_LOCK.lock().unwrap();
        let _sandbox = ConfigSandbox::new();
        let dir = config_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[api]\nmodel = \"gpt-test\"\n\n[ui]\nmarkdown = false\n",
        )
        .unwrap();
        persist_theme("dark-colorblind").unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: toml::Value = toml::from_str(&raw).unwrap();
        assert_eq!(parsed["api"]["model"].as_str(), Some("gpt-test"));
        assert_eq!(parsed["ui"]["markdown"].as_bool(), Some(false));
        assert_eq!(
            parsed["ui"]["theme"].as_str(),
            Some("dark-colorblind")
        );
    }

    #[test]
    fn persist_theme_creates_file_when_absent() {
        let _g = ENV_LOCK.lock().unwrap();
        let _sandbox = ConfigSandbox::new();
        persist_theme("light-ansi").unwrap();
        let raw = std::fs::read_to_string(config_dir().unwrap().join("config.toml")).unwrap();
        assert!(raw.contains("[ui]"));
        assert!(raw.contains("light-ansi"));
    }

    #[test]
    fn render_preview_height_is_stable_across_themes() {
        for name in Theme::all_names() {
            let theme = Theme::from_name(name);
            let body = render_preview(&theme);
            assert_eq!(body.matches('\n').count(), 5, "theme {name}");
        }
    }

    #[test]
    fn picker_options_match_supported_theme_names() {
        let known: std::collections::HashSet<&str> = Theme::all_names().iter().copied().collect();
        for opt in THEME_OPTIONS {
            assert!(
                known.contains(opt.value),
                "picker option {:?} is not in Theme::all_names()",
                opt.value
            );
        }
    }
}
