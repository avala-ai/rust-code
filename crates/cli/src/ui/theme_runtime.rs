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

/// Options recorded alongside the active theme so we can replay the
/// "auto + inherit_fg" override path when callers ask for `current()`.
/// Stored as a separate slot rather than baked into the [`Theme`]
/// struct because the override is dynamic — the OSC 10 cache may
/// populate after `init` has already run.
#[derive(Debug, Clone, Copy, Default)]
struct ActiveOptions {
    /// True when the user's configured theme name was `auto` — the
    /// only situation in which `inherit_fg` should adjust the `text`
    /// slot. Other themes are explicit choices and we honour them
    /// verbatim.
    auto: bool,
    /// True when `[ui].inherit_fg = true` in config.
    inherit_fg: bool,
}

static ACTIVE_THEME: RwLock<Option<Theme>> = RwLock::new(None);
static ACTIVE_OPTIONS: RwLock<ActiveOptions> = RwLock::new(ActiveOptions {
    auto: false,
    inherit_fg: false,
});

/// Initialize (or re-set) the global theme. Convenience wrapper that
/// disables the inherit-fg override; callers that want the override
/// should reach for [`init_with_options`] and pass the configured
/// (pre-resolution) theme name plus the `inherit_fg` flag.
pub fn init(theme_name: &str) {
    init_with_options(theme_name, theme_name, false);
}

/// Initialize the global theme with the inherit-fg override.
///
/// `configured_name` is the *user-typed* theme value (`"auto"`,
/// `"midnight"`, …); `theme_name` is what [`resolve_theme`] returned
/// for it. We need both because `inherit_fg` only fires when the
/// user opted into Auto — explicit themes are taken at face value.
pub fn init_with_options(theme_name: &str, configured_name: &str, inherit_fg: bool) {
    let theme = Theme::from_name(theme_name);
    if let Ok(mut guard) = ACTIVE_THEME.write() {
        *guard = Some(theme);
    }
    if let Ok(mut guard) = ACTIVE_OPTIONS.write() {
        *guard = ActiveOptions {
            auto: configured_name == "auto",
            inherit_fg,
        };
    }
}

/// Get a snapshot of the active theme. Each color slot is adapted to
/// the current [`color_emit::EmitMode`] so consumers transparently get
/// the right palette without threading the mode through every render
/// callsite. The stored theme keeps its original RGB values; only this
/// snapshot is downgraded.
///
/// When the user picked Auto and enabled `inherit_fg`, the `text`
/// slot is replaced with the foreground RGB the terminal reported via
/// OSC 10. If detection failed (cache empty) the theme default is
/// preserved instead.
pub fn current() -> Theme {
    let raw = ACTIVE_THEME
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(Theme::midnight);
    let options = ACTIVE_OPTIONS.read().map(|g| *g).unwrap_or_default();
    adapt_for_emit(apply_inherit_fg(raw, options))
}

fn apply_inherit_fg(mut theme: Theme, options: ActiveOptions) -> Theme {
    apply_inherit_fg_with(
        &mut theme,
        options,
        super::terminal_query::detect_terminal_foreground(),
    );
    theme
}

/// Pure variant of [`apply_inherit_fg`] used by both the live path and
/// the unit tests. Splitting the override away from the cache lookup
/// is the only way to exercise the "detection succeeded / detection
/// failed / inherit-fg disabled" matrix without racing on a global
/// `OnceLock`.
fn apply_inherit_fg_with(
    theme: &mut Theme,
    options: ActiveOptions,
    detected: Option<(u8, u8, u8)>,
) {
    if options.auto
        && options.inherit_fg
        && let Some((r, g, b)) = detected
    {
        theme.text = Color::Rgb { r, g, b };
    }
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

    /// Helper: build the `ActiveOptions` matrix value once.
    fn opts(auto: bool, inherit_fg: bool) -> ActiveOptions {
        ActiveOptions { auto, inherit_fg }
    }

    #[test]
    fn inherit_fg_overrides_text_when_detection_succeeds() {
        // Auto + inherit_fg + cached foreground → text slot becomes
        // the detected RGB, every other slot is left alone.
        let mut theme = Theme::midnight();
        let original_accent = theme.accent;
        apply_inherit_fg_with(&mut theme, opts(true, true), Some((0x12, 0x34, 0x56)));
        assert_eq!(
            theme.text,
            Color::Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        );
        // Sibling slots untouched — the override is fg-only.
        assert_eq!(theme.accent, original_accent);
    }

    #[test]
    fn inherit_fg_falls_back_to_theme_text_when_detection_fails() {
        // Auto + inherit_fg but no cached foreground → keep the
        // theme's own text colour. This is the "we didn't get an OSC
        // 10 reply" branch.
        let mut theme = Theme::midnight();
        let default_text = theme.text;
        apply_inherit_fg_with(&mut theme, opts(true, true), None);
        assert_eq!(theme.text, default_text);
    }

    #[test]
    fn inherit_fg_disabled_keeps_theme_default_even_with_cache_hit() {
        // Auto theme but inherit_fg = false → ignore the cache. This
        // is the default behaviour and protects users who explicitly
        // chose Auto without opting into the override.
        let mut theme = Theme::midnight();
        let default_text = theme.text;
        apply_inherit_fg_with(&mut theme, opts(true, false), Some((0xAA, 0xBB, 0xCC)));
        assert_eq!(theme.text, default_text);
    }

    #[test]
    fn inherit_fg_only_fires_when_configured_theme_was_auto() {
        // Explicit theme + inherit_fg = true → still ignore the
        // cache. The override is Auto-only because users who picked
        // Midnight (etc.) intentionally signed up for that palette.
        let mut theme = Theme::midnight();
        let default_text = theme.text;
        apply_inherit_fg_with(&mut theme, opts(false, true), Some((0xAA, 0xBB, 0xCC)));
        assert_eq!(theme.text, default_text);
    }
}
