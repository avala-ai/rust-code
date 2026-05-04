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

    // Shimmer variants — see [`legacy::Theme`] for slot semantics.
    pub success_shimmer: Color,
    pub error_shimmer: Color,
    pub warning_shimmer: Color,
    pub accent_shimmer: Color,
    pub muted_shimmer: Color,

    // Diff intensity variants.
    pub diff_added_dimmed: Color,
    pub diff_removed_dimmed: Color,
    pub diff_added_word: Color,
    pub diff_removed_word: Color,

    // Stable subagent identification colors.
    pub subagent_red: Color,
    pub subagent_blue: Color,
    pub subagent_green: Color,
    pub subagent_yellow: Color,
    pub subagent_purple: Color,
    pub subagent_orange: Color,
    pub subagent_pink: Color,
    pub subagent_cyan: Color,

    // Rate-limit / budget bar.
    pub rate_limit_fill: Color,
    pub rate_limit_empty: Color,

    // Selection / hover / interaction backgrounds.
    pub selection_bg: Color,
    pub message_action_bg: Color,
    pub user_message_bg: Color,
    pub bash_message_bg: Color,
    pub memory_message_bg: Color,

    // Rainbow keyword highlighting.
    pub rainbow_red: Color,
    pub rainbow_orange: Color,
    pub rainbow_yellow: Color,
    pub rainbow_green: Color,
    pub rainbow_blue: Color,
    pub rainbow_indigo: Color,
    pub rainbow_violet: Color,

    // Mode tags for the per-mode REPL prompt.
    pub plan_mode: Color,
    pub brief_mode: Color,
    pub fast_mode: Color,
    pub fast_mode_shimmer: Color,

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
            success_shimmer: theme.success_shimmer,
            error_shimmer: theme.error_shimmer,
            warning_shimmer: theme.warning_shimmer,
            accent_shimmer: theme.accent_shimmer,
            muted_shimmer: theme.muted_shimmer,
            diff_added_dimmed: theme.diff_added_dimmed,
            diff_removed_dimmed: theme.diff_removed_dimmed,
            diff_added_word: theme.diff_added_word,
            diff_removed_word: theme.diff_removed_word,
            subagent_red: theme.subagent_red,
            subagent_blue: theme.subagent_blue,
            subagent_green: theme.subagent_green,
            subagent_yellow: theme.subagent_yellow,
            subagent_purple: theme.subagent_purple,
            subagent_orange: theme.subagent_orange,
            subagent_pink: theme.subagent_pink,
            subagent_cyan: theme.subagent_cyan,
            rate_limit_fill: theme.rate_limit_fill,
            rate_limit_empty: theme.rate_limit_empty,
            selection_bg: theme.selection_bg,
            message_action_bg: theme.message_action_bg,
            user_message_bg: theme.user_message_bg,
            bash_message_bg: theme.bash_message_bg,
            memory_message_bg: theme.memory_message_bg,
            rainbow_red: theme.rainbow_red,
            rainbow_orange: theme.rainbow_orange,
            rainbow_yellow: theme.rainbow_yellow,
            rainbow_green: theme.rainbow_green,
            rainbow_blue: theme.rainbow_blue,
            rainbow_indigo: theme.rainbow_indigo,
            rainbow_violet: theme.rainbow_violet,
            plan_mode: theme.plan_mode,
            brief_mode: theme.brief_mode,
            fast_mode: theme.fast_mode,
            fast_mode_shimmer: theme.fast_mode_shimmer,
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

/// Get a snapshot of the active theme. Each color slot is adapted to
/// the current [`color_emit::EmitMode`] so consumers transparently get
/// the right palette without threading the mode through every render
/// callsite. The stored theme keeps its original RGB values; only this
/// snapshot is downgraded.
pub fn current() -> Theme {
    let raw = ACTIVE_THEME
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(Theme::midnight);
    adapt_for_emit(raw)
}

fn adapt_for_emit(theme: Theme) -> Theme {
    let mode = super::color_emit::current();
    if mode == super::color_emit::EmitMode::Truecolor {
        return theme;
    }
    let adapt = |c: Color| super::color_emit::adapt(mode, c);
    Theme {
        accent: adapt(theme.accent),
        error: adapt(theme.error),
        warning: adapt(theme.warning),
        success: adapt(theme.success),
        muted: adapt(theme.muted),
        inactive: adapt(theme.inactive),
        tool: adapt(theme.tool),
        plan: adapt(theme.plan),
        text: adapt(theme.text),
        diff_add: adapt(theme.diff_add),
        diff_remove: adapt(theme.diff_remove),
        agent_colors: theme.agent_colors.map(adapt),
        success_shimmer: adapt(theme.success_shimmer),
        error_shimmer: adapt(theme.error_shimmer),
        warning_shimmer: adapt(theme.warning_shimmer),
        accent_shimmer: adapt(theme.accent_shimmer),
        muted_shimmer: adapt(theme.muted_shimmer),
        diff_added_dimmed: adapt(theme.diff_added_dimmed),
        diff_removed_dimmed: adapt(theme.diff_removed_dimmed),
        diff_added_word: adapt(theme.diff_added_word),
        diff_removed_word: adapt(theme.diff_removed_word),
        subagent_red: adapt(theme.subagent_red),
        subagent_blue: adapt(theme.subagent_blue),
        subagent_green: adapt(theme.subagent_green),
        subagent_yellow: adapt(theme.subagent_yellow),
        subagent_purple: adapt(theme.subagent_purple),
        subagent_orange: adapt(theme.subagent_orange),
        subagent_pink: adapt(theme.subagent_pink),
        subagent_cyan: adapt(theme.subagent_cyan),
        rate_limit_fill: adapt(theme.rate_limit_fill),
        rate_limit_empty: adapt(theme.rate_limit_empty),
        selection_bg: adapt(theme.selection_bg),
        message_action_bg: adapt(theme.message_action_bg),
        user_message_bg: adapt(theme.user_message_bg),
        bash_message_bg: adapt(theme.bash_message_bg),
        memory_message_bg: adapt(theme.memory_message_bg),
        rainbow_red: adapt(theme.rainbow_red),
        rainbow_orange: adapt(theme.rainbow_orange),
        rainbow_yellow: adapt(theme.rainbow_yellow),
        rainbow_green: adapt(theme.rainbow_green),
        rainbow_blue: adapt(theme.rainbow_blue),
        rainbow_indigo: adapt(theme.rainbow_indigo),
        rainbow_violet: adapt(theme.rainbow_violet),
        plan_mode: adapt(theme.plan_mode),
        brief_mode: adapt(theme.brief_mode),
        fast_mode: adapt(theme.fast_mode),
        fast_mode_shimmer: adapt(theme.fast_mode_shimmer),
        is_dark: theme.is_dark,
    }
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
