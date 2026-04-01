//! Auto theme detection.
//!
//! Detects the terminal's background color to choose between
//! light and dark themes. Falls back to dark if detection fails.

/// Detect whether the terminal has a light background.
///
/// Uses the `COLORFGBG` environment variable (set by many terminals)
/// and the `TERM_PROGRAM` heuristic. Falls back to dark.
pub fn detect_system_theme() -> &'static str {
    // Method 1: COLORFGBG env var (e.g., "15;0" means white-on-black = dark).
    if let Ok(fgbg) = std::env::var("COLORFGBG")
        && let Some(bg) = fgbg.rsplit(';').next()
        && let Ok(bg_num) = bg.parse::<u32>()
    {
        // Standard terminal colors: 0-6 are dark, 7+ are light.
        return if bg_num >= 7 && bg_num != 8 {
            "light"
        } else {
            "dark"
        };
    }

    // Method 2: macOS Terminal.app defaults to light.
    if std::env::var("TERM_PROGRAM")
        .ok()
        .is_some_and(|p| p == "Apple_Terminal")
    {
        return "light";
    }

    // Default to dark.
    "dark"
}

/// Resolve the effective theme name, handling "auto" by detecting the system theme.
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
        // Auto should resolve to either midnight or daybreak.
        let result = resolve_theme("auto");
        assert!(result == "midnight" || result == "daybreak");
    }
}
