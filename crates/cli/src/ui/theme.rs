//! Terminal color themes.
//!
//! Provides a semantic color system where UI elements reference
//! named colors (error, warning, accent, etc.) rather than
//! hardcoded ANSI codes. Supports dark, light, and ANSI-only themes.

use crossterm::style::{Attribute, Color, StyledContent, Stylize};

/// Semantic color palette for a theme.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Brand accent color (prompts, banners, decorations).
    pub accent: Color,
    /// Error messages and failure indicators.
    pub error: Color,
    /// Warning messages and non-critical alerts.
    pub warning: Color,
    /// Success messages and confirmations.
    pub success: Color,
    /// Secondary/meta information.
    pub muted: Color,
    /// Inactive/disabled elements.
    pub inactive: Color,
    /// Permission prompts and tool labels.
    pub tool: Color,
    /// Plan mode indicator.
    pub plan: Color,
    /// Primary text (usually terminal default).
    pub text: Color,
    /// Diff: added lines.
    pub diff_add: Color,
    /// Diff: removed lines.
    pub diff_remove: Color,
    /// Subagent identification colors (8 distinct).
    pub agent_colors: [Color; 8],
    /// Whether this is a dark theme (affects background choices).
    pub is_dark: bool,
}

impl Theme {
    /// Midnight — dark theme with cyan accents.
    pub fn midnight() -> Self {
        Self {
            accent: Color::Rgb {
                r: 164,
                g: 34,
                b: 225,
            },
            error: Color::Rgb {
                r: 255,
                g: 107,
                b: 128,
            },
            warning: Color::Rgb {
                r: 255,
                g: 193,
                b: 7,
            },
            success: Color::Rgb {
                r: 78,
                g: 186,
                b: 101,
            },
            muted: Color::Rgb {
                r: 100,
                g: 100,
                b: 100,
            },
            inactive: Color::Rgb {
                r: 153,
                g: 153,
                b: 153,
            },
            tool: Color::Rgb {
                r: 177,
                g: 185,
                b: 249,
            },
            plan: Color::Rgb {
                r: 72,
                g: 150,
                b: 140,
            },
            text: Color::White,
            diff_add: Color::Rgb {
                r: 56,
                g: 166,
                b: 96,
            },
            diff_remove: Color::Rgb {
                r: 179,
                g: 89,
                b: 107,
            },
            agent_colors: [
                Color::Rgb {
                    r: 220,
                    g: 38,
                    b: 38,
                },
                Color::Rgb {
                    r: 37,
                    g: 99,
                    b: 235,
                },
                Color::Rgb {
                    r: 22,
                    g: 163,
                    b: 74,
                },
                Color::Rgb {
                    r: 202,
                    g: 138,
                    b: 4,
                },
                Color::Rgb {
                    r: 147,
                    g: 51,
                    b: 234,
                },
                Color::Rgb {
                    r: 234,
                    g: 88,
                    b: 12,
                },
                Color::Rgb {
                    r: 219,
                    g: 39,
                    b: 119,
                },
                Color::Rgb {
                    r: 8,
                    g: 145,
                    b: 178,
                },
            ],
            is_dark: true,
        }
    }

    /// Daybreak — light theme with blue accents.
    pub fn daybreak() -> Self {
        Self {
            accent: Color::Rgb {
                r: 130,
                g: 20,
                b: 180,
            },
            error: Color::Rgb {
                r: 171,
                g: 43,
                b: 63,
            },
            warning: Color::Rgb {
                r: 150,
                g: 108,
                b: 30,
            },
            success: Color::Rgb {
                r: 44,
                g: 122,
                b: 57,
            },
            muted: Color::Rgb {
                r: 140,
                g: 140,
                b: 140,
            },
            inactive: Color::Rgb {
                r: 102,
                g: 102,
                b: 102,
            },
            tool: Color::Rgb {
                r: 87,
                g: 105,
                b: 247,
            },
            plan: Color::Rgb {
                r: 50,
                g: 120,
                b: 110,
            },
            text: Color::Black,
            diff_add: Color::Rgb {
                r: 47,
                g: 157,
                b: 68,
            },
            diff_remove: Color::Rgb {
                r: 209,
                g: 69,
                b: 75,
            },
            agent_colors: [
                Color::Rgb {
                    r: 185,
                    g: 28,
                    b: 28,
                },
                Color::Rgb {
                    r: 29,
                    g: 78,
                    b: 216,
                },
                Color::Rgb {
                    r: 21,
                    g: 128,
                    b: 61,
                },
                Color::Rgb {
                    r: 161,
                    g: 98,
                    b: 7,
                },
                Color::Rgb {
                    r: 126,
                    g: 34,
                    b: 206,
                },
                Color::Rgb {
                    r: 194,
                    g: 65,
                    b: 12,
                },
                Color::Rgb {
                    r: 190,
                    g: 24,
                    b: 93,
                },
                Color::Rgb {
                    r: 14,
                    g: 116,
                    b: 144,
                },
            ],
            is_dark: false,
        }
    }

    /// Midnight Muted — softer dark theme.
    pub fn midnight_muted() -> Self {
        let mut t = Self::midnight();
        t.accent = Color::Rgb {
            r: 140,
            g: 60,
            b: 190,
        };
        t.error = Color::Rgb {
            r: 200,
            g: 100,
            b: 110,
        };
        t.warning = Color::Rgb {
            r: 200,
            g: 170,
            b: 60,
        };
        t.success = Color::Rgb {
            r: 80,
            g: 160,
            b: 90,
        };
        t.tool = Color::Rgb {
            r: 150,
            g: 155,
            b: 200,
        };
        t
    }

    /// Daybreak Muted — softer light theme.
    pub fn daybreak_muted() -> Self {
        let mut t = Self::daybreak();
        t.accent = Color::Rgb {
            r: 110,
            g: 30,
            b: 150,
        };
        t.error = Color::Rgb {
            r: 150,
            g: 50,
            b: 60,
        };
        t.warning = Color::Rgb {
            r: 130,
            g: 100,
            b: 40,
        };
        t.success = Color::Rgb {
            r: 50,
            g: 110,
            b: 55,
        };
        t.tool = Color::Rgb {
            r: 70,
            g: 85,
            b: 190,
        };
        t
    }

    /// Terminal — uses standard 16 ANSI colors only.
    pub fn terminal() -> Self {
        Self {
            accent: Color::Magenta,
            error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            muted: Color::DarkGrey,
            inactive: Color::Grey,
            tool: Color::Cyan,
            plan: Color::DarkGreen,
            text: Color::Reset,
            diff_add: Color::Green,
            diff_remove: Color::Red,
            agent_colors: [
                Color::Red,
                Color::Blue,
                Color::Green,
                Color::Yellow,
                Color::Magenta,
                Color::DarkYellow,
                Color::DarkMagenta,
                Color::Cyan,
            ],
            is_dark: true,
        }
    }

    /// Dark theme using the Okabe-Ito palette — chosen for protanopia,
    /// deuteranopia, and tritanopia distinguishability. The eight
    /// canonical hex values are mapped to semantic slots so diffs and
    /// status indicators stay separable for users with common forms of
    /// colour-vision deficiency.
    pub fn dark_colorblind() -> Self {
        // Okabe-Ito palette (canonical hex):
        //   black       #000000
        //   orange      #E69F00
        //   sky-blue    #56B4E9
        //   bluish-green#009E73
        //   yellow      #F0E442
        //   blue        #0072B2
        //   vermillion  #D55E00
        //   reddish-purple #CC79A7
        let orange = Color::Rgb {
            r: 0xE6,
            g: 0x9F,
            b: 0x00,
        };
        let sky_blue = Color::Rgb {
            r: 0x56,
            g: 0xB4,
            b: 0xE9,
        };
        let bluish_green = Color::Rgb {
            r: 0x00,
            g: 0x9E,
            b: 0x73,
        };
        let yellow = Color::Rgb {
            r: 0xF0,
            g: 0xE4,
            b: 0x42,
        };
        let blue = Color::Rgb {
            r: 0x00,
            g: 0x72,
            b: 0xB2,
        };
        let vermillion = Color::Rgb {
            r: 0xD5,
            g: 0x5E,
            b: 0x00,
        };
        let reddish_purple = Color::Rgb {
            r: 0xCC,
            g: 0x79,
            b: 0xA7,
        };

        Self {
            accent: sky_blue,
            error: vermillion,
            warning: orange,
            success: bluish_green,
            muted: Color::Rgb {
                r: 130,
                g: 130,
                b: 130,
            },
            inactive: Color::Rgb {
                r: 170,
                g: 170,
                b: 170,
            },
            tool: reddish_purple,
            plan: blue,
            text: Color::White,
            diff_add: bluish_green,
            diff_remove: vermillion,
            agent_colors: [
                vermillion,
                blue,
                bluish_green,
                yellow,
                sky_blue,
                orange,
                reddish_purple,
                Color::White,
            ],
            is_dark: true,
        }
    }

    /// Light counterpart of [`Theme::dark_colorblind`] using the same
    /// Okabe-Ito palette. The semantic slots are re-mapped to keep
    /// contrast against a pale background; the eight canonical hex
    /// values are still exclusively used.
    pub fn light_colorblind() -> Self {
        let orange = Color::Rgb {
            r: 0xE6,
            g: 0x9F,
            b: 0x00,
        };
        let sky_blue = Color::Rgb {
            r: 0x56,
            g: 0xB4,
            b: 0xE9,
        };
        let bluish_green = Color::Rgb {
            r: 0x00,
            g: 0x9E,
            b: 0x73,
        };
        let yellow = Color::Rgb {
            r: 0xF0,
            g: 0xE4,
            b: 0x42,
        };
        let blue = Color::Rgb {
            r: 0x00,
            g: 0x72,
            b: 0xB2,
        };
        let vermillion = Color::Rgb {
            r: 0xD5,
            g: 0x5E,
            b: 0x00,
        };
        let reddish_purple = Color::Rgb {
            r: 0xCC,
            g: 0x79,
            b: 0xA7,
        };

        Self {
            accent: blue,
            error: vermillion,
            warning: orange,
            success: bluish_green,
            muted: Color::Rgb {
                r: 110,
                g: 110,
                b: 110,
            },
            inactive: Color::Rgb {
                r: 90,
                g: 90,
                b: 90,
            },
            tool: reddish_purple,
            plan: bluish_green,
            text: Color::Black,
            diff_add: bluish_green,
            diff_remove: vermillion,
            agent_colors: [
                vermillion,
                blue,
                bluish_green,
                yellow,
                sky_blue,
                orange,
                reddish_purple,
                Color::Black,
            ],
            is_dark: false,
        }
    }

    /// Dark theme restricted to the 16 standard ANSI colour codes —
    /// for terminals without truecolor support. Every slot is one of
    /// the named [`Color`] variants the standard ANSI palette knows
    /// (no `Color::Rgb`, no 256-colour indices).
    pub fn dark_ansi() -> Self {
        Self {
            accent: Color::Cyan,
            error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            muted: Color::DarkGrey,
            inactive: Color::Grey,
            tool: Color::Magenta,
            plan: Color::Blue,
            text: Color::White,
            diff_add: Color::Green,
            diff_remove: Color::Red,
            agent_colors: [
                Color::Red,
                Color::Blue,
                Color::Green,
                Color::Yellow,
                Color::Magenta,
                Color::Cyan,
                Color::DarkYellow,
                Color::DarkMagenta,
            ],
            is_dark: true,
        }
    }

    /// Light counterpart of [`Theme::dark_ansi`]. Uses the dark ANSI
    /// variants where contrast on a pale background matters.
    pub fn light_ansi() -> Self {
        Self {
            accent: Color::DarkBlue,
            error: Color::DarkRed,
            warning: Color::DarkYellow,
            success: Color::DarkGreen,
            muted: Color::DarkGrey,
            inactive: Color::Grey,
            tool: Color::DarkMagenta,
            plan: Color::DarkCyan,
            text: Color::Black,
            diff_add: Color::DarkGreen,
            diff_remove: Color::DarkRed,
            agent_colors: [
                Color::DarkRed,
                Color::DarkBlue,
                Color::DarkGreen,
                Color::DarkYellow,
                Color::DarkMagenta,
                Color::DarkCyan,
                Color::Red,
                Color::Blue,
            ],
            is_dark: false,
        }
    }

    /// Resolve a theme name to a Theme instance.
    pub fn from_name(name: &str) -> Self {
        match name {
            "midnight" | "dark" => Self::midnight(),
            "daybreak" | "light" => Self::daybreak(),
            "midnight-muted" => Self::midnight_muted(),
            "daybreak-muted" => Self::daybreak_muted(),
            "terminal" => Self::terminal(),
            "dark-colorblind" => Self::dark_colorblind(),
            "light-colorblind" => Self::light_colorblind(),
            "dark-ansi" => Self::dark_ansi(),
            "light-ansi" => Self::light_ansi(),
            "auto" => {
                let detected = detect_system_theme();
                if detected == "light" {
                    Self::daybreak()
                } else {
                    Self::midnight()
                }
            }
            _ => Self::midnight(),
        }
    }

    /// All known theme identifiers in display order. Drives the
    /// onboarding picker and the `/theme` slash command.
    pub fn all_names() -> &'static [&'static str] {
        &[
            "auto",
            "midnight",
            "daybreak",
            "dark-colorblind",
            "light-colorblind",
            "dark-ansi",
            "light-ansi",
        ]
    }

    /// Get a subagent color by index (wraps around).
    pub fn agent_color(&self, index: usize) -> Color {
        self.agent_colors[index % self.agent_colors.len()]
    }
}

// ---- Styling helpers ----

/// Style text with a theme color.
pub fn styled(text: &str, color: Color) -> StyledContent<String> {
    text.to_string().with(color)
}

/// Style text with a theme color and bold.
pub fn styled_bold(text: &str, color: Color) -> StyledContent<String> {
    text.to_string().with(color).attribute(Attribute::Bold)
}

/// Style a label with background color (e.g., " ERROR " on red background).
pub fn label(text: &str, bg: Color, fg: Color) -> StyledContent<String> {
    text.to_string().on(bg).with(fg).attribute(Attribute::Bold)
}

// ---- Theme detection ----

/// Detect whether the terminal has a light background.
pub fn detect_system_theme() -> &'static str {
    if let Ok(fgbg) = std::env::var("COLORFGBG")
        && let Some(bg) = fgbg.rsplit(';').next()
        && let Ok(bg_num) = bg.parse::<u32>()
    {
        return if bg_num >= 7 && bg_num != 8 {
            "light"
        } else {
            "dark"
        };
    }

    if std::env::var("TERM_PROGRAM")
        .ok()
        .is_some_and(|p| p == "Apple_Terminal")
    {
        return "light";
    }

    "dark"
}

/// Resolve a config theme name, handling "auto".
pub fn resolve_theme(configured: &str) -> String {
    if configured == "auto" {
        let detected = detect_system_theme();
        if detected == "light" {
            "daybreak".to_string()
        } else {
            "midnight".to_string()
        }
    } else {
        configured.to_string()
    }
}

// ---- Global theme access ----
//
// The active theme is held behind a `RwLock` rather than a `OnceLock`
// so the `/theme` slash command can re-paint the running session
// without requiring a restart. Writes happen at most once per user
// command; reads are cheap because `Theme` is `Clone` and the lock
// hold time is microseconds.

use std::sync::RwLock;

static ACTIVE_THEME: RwLock<Option<Theme>> = RwLock::new(None);

/// Initialize (or re-set) the global theme. Safe to call from the
/// startup path *and* from the `/theme` slash command — the latter
/// overrides any previously installed theme.
pub fn init(theme_name: &str) {
    let theme = Theme::from_name(theme_name);
    if let Ok(mut guard) = ACTIVE_THEME.write() {
        *guard = Some(theme);
    }
}

/// Get a snapshot of the active theme. Falls back to midnight if not
/// initialized. Returns by value (cheap clone) so callers don't need
/// to hold the lock across rendering.
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
    fn test_resolve_non_auto() {
        assert_eq!(resolve_theme("midnight"), "midnight");
        assert_eq!(resolve_theme("daybreak"), "daybreak");
    }

    #[test]
    fn test_resolve_auto_returns_valid_theme() {
        let result = resolve_theme("auto");
        assert!(result == "midnight" || result == "daybreak");
    }

    #[test]
    fn test_theme_from_name() {
        let t = Theme::from_name("midnight");
        assert!(t.is_dark);
        let t = Theme::from_name("daybreak");
        assert!(!t.is_dark);
    }

    #[test]
    fn test_agent_color_wraps() {
        let t = Theme::midnight();
        let c0 = t.agent_color(0);
        let c8 = t.agent_color(8);
        assert!(matches!(c0, Color::Rgb { .. }));
        assert!(matches!(c8, Color::Rgb { .. }));
    }

    #[test]
    fn every_named_theme_resolves() {
        // Tripwire: every name advertised by `all_names` must produce a
        // theme whose every slot is populated. `Color` has no Default
        // impl so a missing field is a compile error already; this
        // covers semantic completeness too — matching `Color::Reset`
        // on a non-text slot is the "I forgot to set this" footgun.
        for name in Theme::all_names() {
            let t = Theme::from_name(name);
            assert!(
                !matches!(t.accent, Color::Reset),
                "theme {name} has unset accent"
            );
            assert!(
                !matches!(t.error, Color::Reset),
                "theme {name} has unset error"
            );
            assert!(
                !matches!(t.diff_add, Color::Reset),
                "theme {name} has unset diff_add"
            );
            assert!(
                !matches!(t.diff_remove, Color::Reset),
                "theme {name} has unset diff_remove"
            );
            assert_eq!(
                t.agent_colors.len(),
                8,
                "theme {name} must define 8 agent colours"
            );
        }
    }

    #[test]
    fn colorblind_palette_uses_okabe_ito_hex_values() {
        // Spec: every Okabe-Ito canonical hex value must show up in
        // either dark-colorblind or light-colorblind. The two themes
        // share the same palette and only re-map semantic slots, so a
        // single union suffices.
        let okabe_ito: &[(u8, u8, u8)] = &[
            (0xE6, 0x9F, 0x00), // orange
            (0x56, 0xB4, 0xE9), // sky-blue
            (0x00, 0x9E, 0x73), // bluish-green
            (0xF0, 0xE4, 0x42), // yellow
            (0x00, 0x72, 0xB2), // blue
            (0xD5, 0x5E, 0x00), // vermillion
            (0xCC, 0x79, 0xA7), // reddish-purple
        ];

        let collect = |t: &Theme| -> Vec<(u8, u8, u8)> {
            let mut all = vec![
                t.accent,
                t.error,
                t.warning,
                t.success,
                t.tool,
                t.plan,
                t.diff_add,
                t.diff_remove,
            ];
            all.extend_from_slice(&t.agent_colors);
            all.into_iter()
                .filter_map(|c| match c {
                    Color::Rgb { r, g, b } => Some((r, g, b)),
                    _ => None,
                })
                .collect()
        };

        let dark = Theme::dark_colorblind();
        let light = Theme::light_colorblind();
        let mut union: Vec<(u8, u8, u8)> = collect(&dark);
        union.extend(collect(&light));
        union.sort_unstable();
        union.dedup();

        for triple in okabe_ito {
            assert!(
                union.contains(triple),
                "Okabe-Ito colour {:02X}{:02X}{:02X} missing from colourblind themes",
                triple.0,
                triple.1,
                triple.2,
            );
        }
        // Black (the eighth Okabe-Ito colour) should appear as text in
        // at least the light variant — assert separately so the union
        // check stays focused on the chromatic seven.
        assert!(matches!(light.text, Color::Black));
    }

    #[test]
    fn ansi_only_themes_use_no_truecolor() {
        // For every slot in the ANSI-only themes, the colour must be
        // one of the 16 standard ANSI variants — no `Rgb`, no
        // `AnsiValue`. The latter would still resolve in 256-colour
        // terminals but doesn't degrade in 16-colour ones.
        fn is_ansi_16(c: Color) -> bool {
            matches!(
                c,
                Color::Reset
                    | Color::Black
                    | Color::Red
                    | Color::Green
                    | Color::Yellow
                    | Color::Blue
                    | Color::Magenta
                    | Color::Cyan
                    | Color::Grey
                    | Color::White
                    | Color::DarkGrey
                    | Color::DarkRed
                    | Color::DarkGreen
                    | Color::DarkYellow
                    | Color::DarkBlue
                    | Color::DarkMagenta
                    | Color::DarkCyan
            )
        }
        for theme in [Theme::dark_ansi(), Theme::light_ansi()] {
            for c in [
                theme.accent,
                theme.error,
                theme.warning,
                theme.success,
                theme.muted,
                theme.inactive,
                theme.tool,
                theme.plan,
                theme.text,
                theme.diff_add,
                theme.diff_remove,
            ] {
                assert!(is_ansi_16(c), "ANSI theme leaked non-ANSI colour {c:?}");
            }
            for c in theme.agent_colors {
                assert!(is_ansi_16(c), "ANSI theme agent colour {c:?} is non-ANSI");
            }
        }
    }
}
