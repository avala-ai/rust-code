//! Runtime facade for terminal themes.

use std::sync::RwLock;

use crossterm::style::{Color, StyledContent};

#[path = "theme.rs"]
mod legacy;

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub muted: Color,
    pub inactive: Color,
    pub tool: Color,
    pub plan: Color,
    pub text: Color,
    pub diff_add: Color,
    pub diff_remove: Color,
    pub agent_colors: [Color; 8],
    pub is_dark: bool,
}

impl Theme {
    pub fn midnight() -> Self {
        legacy::Theme::midnight().into()
    }

    pub fn daybreak() -> Self {
        legacy::Theme::daybreak().into()
    }

    pub fn midnight_muted() -> Self {
        legacy::Theme::midnight_muted().into()
    }

    pub fn daybreak_muted() -> Self {
        legacy::Theme::daybreak_muted().into()
    }

    pub fn terminal() -> Self {
        legacy::Theme::terminal().into()
    }

    pub fn dark_colorblind() -> Self {
        legacy::Theme::dark_colorblind().into()
    }

    pub fn light_colorblind() -> Self {
        legacy::Theme::light_colorblind().into()
    }

    pub fn dark_ansi() -> Self {
        legacy::Theme::dark_ansi().into()
    }

    pub fn light_ansi() -> Self {
        legacy::Theme::light_ansi().into()
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "auto" => {
                if detect_system_theme() == "light" {
                    Self::daybreak()
                } else {
                    Self::midnight()
                }
            }
            _ => legacy::Theme::from_name(name).into(),
        }
    }

    pub fn all_names() -> &'static [&'static str] {
        legacy::Theme::all_names()
    }

    pub fn agent_color(&self, index: usize) -> Color {
        self.agent_colors[index % self.agent_colors.len()]
    }
}

impl From<legacy::Theme> for Theme {
    fn from(theme: legacy::Theme) -> Self {
        Self {
            accent: theme.accent,
            error: theme.error,
            warning: theme.warning,
            success: theme.success,
            muted: theme.muted,
            inactive: theme.inactive,
            tool: theme.tool,
            plan: theme.plan,
            text: theme.text,
            diff_add: theme.diff_add,
            diff_remove: theme.diff_remove,
            agent_colors: theme.agent_colors,
            is_dark: theme.is_dark,
        }
    }
}

pub fn styled(text: &str, color: Color) -> StyledContent<String> {
    legacy::styled(text, color)
}

pub fn styled_bold(text: &str, color: Color) -> StyledContent<String> {
    legacy::styled_bold(text, color)
}

pub fn label(text: &str, bg: Color, fg: Color) -> StyledContent<String> {
    legacy::label(text, bg, fg)
}

/// Detect whether the terminal has a light background.
pub fn detect_system_theme() -> &'static str {
    super::terminal_query::system_theme().as_str()
}

/// Resolve a config theme name, handling "auto".
pub fn resolve_theme(configured: &str) -> String {
    if configured == "auto" {
        if detect_system_theme() == "light" {
            "daybreak".to_string()
        } else {
            "midnight".to_string()
        }
    } else {
        configured.to_string()
    }
}

static ACTIVE_THEME: RwLock<Option<Theme>> = RwLock::new(None);

/// Initialize (or re-set) the global theme.
pub fn init(theme_name: &str) {
    let theme = Theme::from_name(theme_name);
    if let Ok(mut guard) = ACTIVE_THEME.write() {
        *guard = Some(theme);
    }
}

/// Get a snapshot of the active theme.
pub fn current() -> Theme {
    ACTIVE_THEME
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(Theme::midnight)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_non_auto_preserves_configured_name() {
        assert_eq!(resolve_theme("midnight"), "midnight");
        assert_eq!(resolve_theme("daybreak"), "daybreak");
    }

    #[test]
    fn every_advertised_theme_resolves_through_facade() {
        for name in Theme::all_names() {
            let theme = Theme::from_name(name);
            assert_eq!(theme.agent_colors.len(), 8, "theme {name}");
        }
    }
}
