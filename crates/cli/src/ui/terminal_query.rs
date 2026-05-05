//! Terminal query helpers for theme auto-detection.
//!
//! Wraps OSC 10 (default foreground) and OSC 11 (default background)
//! queries with a single DA1 sentinel so both colours come back in one
//! round-trip. The background reply drives the dark-vs-light decision
//! that picks the Auto theme; the foreground reply is exposed
//! separately for the optional `inherit_fg` mode where the Auto theme
//! reuses the terminal's own foreground for its `text` slot.

use std::io::{self, IsTerminal, Read, Write};
use std::sync::OnceLock;
use std::time::Duration;

#[cfg(unix)]
use crossterm::terminal;

pub const SYSTEM_THEME_ENV: &str = "AGENT_CODE_SYSTEM_THEME";

const OSC_10_QUERY: &[u8] = b"\x1b]10;?\x07";
const OSC_11_QUERY: &[u8] = b"\x1b]11;?\x07";
const DA1_QUERY: &[u8] = b"\x1b[c";
const QUERY_TIMEOUT: Duration = Duration::from_secs(1);

static SYSTEM_THEME: OnceLock<SystemTheme> = OnceLock::new();
static FOREGROUND_RGB: OnceLock<Option<Rgb>> = OnceLock::new();
static BACKGROUND_RGB: OnceLock<Option<Rgb>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemTheme {
    Dark,
    Light,
}

impl SystemTheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    fn from_env(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub fn is_tty() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn system_theme() -> SystemTheme {
    *SYSTEM_THEME.get_or_init(detect_system_theme_uncached)
}

/// Detected terminal foreground colour, if the OSC 10 round-trip
/// succeeded. Reads only from cache — call [`prime_terminal_colors`]
/// (or any function that triggers it, like [`system_theme`]) first.
/// Returns the raw RGB triple; classification into dark/light is left
/// to callers because the foreground hex is the answer they want.
pub fn detect_terminal_foreground() -> Option<(u8, u8, u8)> {
    FOREGROUND_RGB
        .get()
        .copied()
        .flatten()
        .map(|c| (c.r, c.g, c.b))
}

/// Send a single OSC 10 + OSC 11 + DA1 batch to the terminal, parse
/// the replies, and populate both colour caches. Idempotent: the
/// underlying [`OnceLock`]s mean repeat calls reuse the first result
/// even if the second invocation arrives from a different code path.
pub fn prime_terminal_colors() {
    SYSTEM_THEME.get_or_init(detect_system_theme_uncached);
}

fn detect_system_theme_uncached() -> SystemTheme {
    // Resolve the dark/light classification synchronously from env vars
    // when possible. The OSC round-trip below populates the raw RGB
    // caches that `inherit_fg` depends on; running it on every path
    // (not just when env detection fails) is what makes
    // `[ui].inherit_fg = true` work on terminals that also export
    // `COLORFGBG` or `TERM_PROGRAM`. The env-derived classification is
    // preferred over the luminance-derived one when both are available
    // — it's what the user explicitly told us.
    let env_theme = std::env::var(SYSTEM_THEME_ENV)
        .ok()
        .and_then(|v| SystemTheme::from_env(&v))
        .or_else(colorfgbg_theme)
        .or_else(|| {
            if std::env::var("TERM_PROGRAM")
                .ok()
                .is_some_and(|p| p == "Apple_Terminal")
            {
                Some(SystemTheme::Light)
            } else {
                None
            }
        });

    let (fg, bg) = query_terminal_colors().unwrap_or((None, None));
    let _ = FOREGROUND_RGB.set(fg);
    let _ = BACKGROUND_RGB.set(bg);

    env_theme
        .or_else(|| bg.map(theme_for_rgb))
        .unwrap_or(SystemTheme::Dark)
}

fn colorfgbg_theme() -> Option<SystemTheme> {
    let fgbg = std::env::var("COLORFGBG").ok()?;
    let bg = fgbg.rsplit(';').next()?;
    let bg_num = bg.parse::<u32>().ok()?;
    if bg_num >= 7 && bg_num != 8 {
        Some(SystemTheme::Light)
    } else {
        Some(SystemTheme::Dark)
    }
}

/// Issue OSC 10 + OSC 11 + DA1 in one batch and return the parsed
/// `(foreground, background)` colours. Either entry may be `None` if
/// the terminal answered one query but not the other (older Apple
/// Terminal, for instance, replies to OSC 11 but not OSC 10).
#[cfg(unix)]
fn query_terminal_colors() -> Option<(Option<Rgb>, Option<Rgb>)> {
    if !is_tty() {
        return None;
    }

    let raw_was_enabled = terminal::is_raw_mode_enabled().unwrap_or(false);
    if !raw_was_enabled && terminal::enable_raw_mode().is_err() {
        return None;
    }

    let result = (|| {
        let mut stdout = io::stdout();
        stdout.write_all(OSC_10_QUERY).ok()?;
        stdout.write_all(OSC_11_QUERY).ok()?;
        stdout.write_all(DA1_QUERY).ok()?;
        stdout.flush().ok()?;

        let bytes = read_stdin_until_da1(QUERY_TIMEOUT)?;
        Some((parse_osc_color(&bytes, 10), parse_osc_color(&bytes, 11)))
    })();

    if !raw_was_enabled {
        let _ = terminal::disable_raw_mode();
    }

    result
}

#[cfg(not(unix))]
fn query_terminal_colors() -> Option<(Option<Rgb>, Option<Rgb>)> {
    None
}

#[cfg(unix)]
fn read_stdin_until_da1(timeout: Duration) -> Option<Vec<u8>> {
    use std::os::fd::AsRawFd;
    use std::time::Instant;

    let stdin = io::stdin();
    let fd = stdin.as_raw_fd();
    let deadline = Instant::now() + timeout;
    let mut out = Vec::with_capacity(128);

    loop {
        if contains_da1_reply(&out) {
            return Some(out);
        }

        let now = Instant::now();
        if now >= deadline {
            return Some(out);
        }
        let remaining = deadline.saturating_duration_since(now);
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;

        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollfd points to one valid descriptor entry for the duration
        // of the call. poll does not retain the pointer after returning.
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready == 0 {
            return Some(out);
        }
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
        if pollfd.revents & libc::POLLIN == 0 {
            continue;
        }

        let mut buf = [0u8; 128];
        // SAFETY: buf is valid for writes of its full length and fd is the
        // stdin file descriptor borrowed for this synchronous call.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n == 0 {
            return Some(out);
        }
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted || err.kind() == io::ErrorKind::WouldBlock {
                continue;
            }
            return None;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
}

/// Test-only helper: drive the OSC 10/11/DA1 batch over an arbitrary
/// reader/writer pair so unit tests can simulate a terminal without
/// touching real stdin/stdout. Returns `(foreground, background)`.
pub fn query_terminal_colors_from_io<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> Option<(Option<Rgb>, Option<Rgb>)> {
    writer.write_all(OSC_10_QUERY).ok()?;
    writer.write_all(OSC_11_QUERY).ok()?;
    writer.write_all(DA1_QUERY).ok()?;
    writer.flush().ok()?;

    let mut out = Vec::with_capacity(128);
    let mut buf = [0u8; 32];
    loop {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if contains_da1_reply(&out) {
            break;
        }
    }

    Some((parse_osc_color(&out, 10), parse_osc_color(&out, 11)))
}

/// Back-compat shim retained for the OSC 11 background path.
pub fn query_system_theme_from_io<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> Option<SystemTheme> {
    let (_, bg) = query_terminal_colors_from_io(reader, writer)?;
    bg.map(theme_for_rgb)
}

/// Find an OSC `code` reply (`code` is 10 for foreground, 11 for
/// background) inside a terminal byte stream and return its parsed
/// RGB. Skips replies whose payload doesn't match a known colour
/// spec, so a malformed first reply does not poison a later valid
/// one.
pub fn parse_osc_color(input: &[u8], code: u32) -> Option<Rgb> {
    let text = std::str::from_utf8(input).ok()?;
    let prefix = format!("\x1b]{code};");
    let mut rest = text;
    while let Some(idx) = rest.find(prefix.as_str()) {
        let payload_start = idx + prefix.len();
        let after_prefix = &rest[payload_start..];
        let payload_end = after_prefix
            .find('\x07')
            .or_else(|| after_prefix.find("\x1b\\"))
            .unwrap_or(after_prefix.len());
        let payload = &after_prefix[..payload_end];
        if let Some(rgb) = parse_color_spec(payload) {
            return Some(rgb);
        }
        rest = &after_prefix[payload_end..];
    }
    parse_color_spec(text.trim())
}

/// Back-compat helper preserved for tests and callers that still want
/// to parse just the OSC 11 reply.
pub fn parse_background_color(input: &[u8]) -> Option<Rgb> {
    parse_osc_color(input, 11)
}

fn parse_color_spec(spec: &str) -> Option<Rgb> {
    let spec = spec.trim();
    if let Some(hex) = spec.strip_prefix('#') {
        return parse_hash_color(hex);
    }
    if let Some(rest) = spec.strip_prefix("rgb:") {
        return parse_component_color(rest, 3);
    }
    if let Some(rest) = spec.strip_prefix("rgba:") {
        return parse_component_color(rest, 4);
    }
    None
}

fn parse_hash_color(hex: &str) -> Option<Rgb> {
    let component_len = match hex.len() {
        3 | 6 | 9 | 12 => hex.len() / 3,
        _ => return None,
    };
    let r = parse_hex_component(&hex[0..component_len])?;
    let g = parse_hex_component(&hex[component_len..component_len * 2])?;
    let b = parse_hex_component(&hex[component_len * 2..component_len * 3])?;
    Some(Rgb { r, g, b })
}

fn parse_component_color(rest: &str, expected_components: usize) -> Option<Rgb> {
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != expected_components {
        return None;
    }
    let r = parse_hex_component(parts[0])?;
    let g = parse_hex_component(parts[1])?;
    let b = parse_hex_component(parts[2])?;
    Some(Rgb { r, g, b })
}

fn parse_hex_component(component: &str) -> Option<u8> {
    if component.is_empty() || component.len() > 4 {
        return None;
    }
    if !component.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(component, 16).ok()?;
    let max = (1u32 << (component.len() * 4)) - 1;
    Some(((value * 255 + (max / 2)) / max) as u8)
}

pub fn theme_for_rgb(rgb: Rgb) -> SystemTheme {
    if bt709_luminance(rgb) >= 0.5 {
        SystemTheme::Light
    } else {
        SystemTheme::Dark
    }
}

pub fn bt709_luminance(rgb: Rgb) -> f64 {
    let r = f64::from(rgb.r) / 255.0;
    let g = f64::from(rgb.g) / 255.0;
    let b = f64::from(rgb.b) / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contains_da1_reply(input: &[u8]) -> bool {
    input.windows(2).enumerate().any(|(idx, bytes)| {
        if bytes != b"\x1b[" {
            return false;
        }
        let tail = &input[idx + 2..];
        let mut consumed = 0usize;
        if tail.first() == Some(&b'?') {
            consumed += 1;
        }
        let mut saw_body = false;
        while let Some(b) = tail.get(consumed) {
            match *b {
                b'0'..=b'9' | b';' => {
                    saw_body = true;
                    consumed += 1;
                }
                b'c' => return saw_body,
                _ => return false,
            }
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn osc(payload: &str) -> Vec<u8> {
        format!("\x1b]11;{payload}\x07").into_bytes()
    }

    fn osc_fg(payload: &str) -> Vec<u8> {
        format!("\x1b]10;{payload}\x07").into_bytes()
    }

    fn rgb(r: u8, g: u8, b: u8) -> Option<Rgb> {
        Some(Rgb { r, g, b })
    }

    #[test]
    fn parses_rgb_components_with_one_to_four_digits() {
        assert_eq!(parse_background_color(&osc("rgb:f/0/8")), rgb(255, 0, 136));
        assert_eq!(
            parse_background_color(&osc("rgb:80/40/20")),
            rgb(128, 64, 32)
        );
        assert_eq!(
            parse_background_color(&osc("rgb:800/400/200")),
            rgb(128, 64, 32)
        );
        assert_eq!(
            parse_background_color(&osc("rgb:8000/4000/2000")),
            rgb(128, 64, 32)
        );
    }

    #[test]
    fn parses_rgba_and_ignores_alpha() {
        assert_eq!(
            parse_background_color(&osc("rgba:ffff/8000/0000/ffff")),
            rgb(255, 128, 0)
        );
    }

    #[test]
    fn parses_hash_forms() {
        assert_eq!(parse_background_color(&osc("#fff")), rgb(255, 255, 255));
        assert_eq!(parse_background_color(&osc("#804020")), rgb(128, 64, 32));
        assert_eq!(parse_background_color(&osc("#800400200")), rgb(128, 64, 32));
        assert_eq!(
            parse_background_color(&osc("#800040002000")),
            rgb(128, 64, 32)
        );
    }

    #[test]
    fn rejects_malformed_color_replies() {
        for payload in [
            "rgb:zzzz/0000/0000",
            "rgb:ffff/0000",
            "rgba:ffff/0000/0000",
            "#12",
            "#12345",
            "not-a-color",
        ] {
            assert_eq!(parse_background_color(&osc(payload)), None, "{payload}");
        }
    }

    #[test]
    fn parses_st_terminated_osc_reply() {
        assert_eq!(
            parse_background_color(b"\x1b]11;rgb:0000/ffff/0000\x1b\\"),
            rgb(0, 255, 0)
        );
    }

    #[test]
    fn parser_distinguishes_osc_10_from_osc_11() {
        // Same byte stream carries both replies; the OSC code selects
        // which payload the parser returns. This is the sentinel
        // property the OSC 10 path needs to be correct.
        let combined = b"\x1b]10;rgb:1010/2020/3030\x07\x1b]11;rgb:f0f0/e0e0/d0d0\x07";
        assert_eq!(parse_osc_color(combined, 10), rgb(0x10, 0x20, 0x30));
        assert_eq!(parse_osc_color(combined, 11), rgb(0xF0, 0xE0, 0xD0));
    }

    #[test]
    fn parses_osc_10_foreground_payload() {
        assert_eq!(
            parse_osc_color(&osc_fg("rgb:abcd/1234/5678"), 10),
            rgb(0xAB, 0x12, 0x56),
        );
        assert_eq!(
            parse_osc_color(&osc_fg("#fa1240"), 10),
            rgb(0xFA, 0x12, 0x40)
        );
        // OSC 10 reply must not be returned when asking for OSC 11.
        assert_eq!(parse_osc_color(&osc_fg("rgb:abcd/1234/5678"), 11), None);
    }

    #[test]
    fn luminance_threshold_classifies_gray_boundary() {
        assert_eq!(
            theme_for_rgb(Rgb {
                r: 127,
                g: 127,
                b: 127
            }),
            SystemTheme::Dark
        );
        assert_eq!(
            theme_for_rgb(Rgb {
                r: 128,
                g: 128,
                b: 128
            }),
            SystemTheme::Light
        );
    }

    #[test]
    fn simulated_query_writes_batch_and_stops_at_da1() {
        // The OSC 10/11/DA1 batch must fire as one round-trip and stop
        // looping the moment the DA1 sentinel is observed — the
        // implementation reads in fixed-size chunks so it may pull a
        // few bytes past the sentinel in the buffer it parses, but it
        // must not block waiting for more input afterwards.
        let mut input = Cursor::new(
            b"\x1b]10;rgb:1010/2020/3030\x07\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?1;2c".to_vec(),
        );
        let mut output = Vec::new();

        let (fg, bg) = query_terminal_colors_from_io(&mut input, &mut output).unwrap();

        assert_eq!(fg, rgb(0x10, 0x20, 0x30));
        assert_eq!(bg, rgb(0xFF, 0xFF, 0xFF));
        // One batch: OSC 10, OSC 11, DA1 — single round-trip.
        assert_eq!(output, b"\x1b]10;?\x07\x1b]11;?\x07\x1b[c");
    }

    #[test]
    fn back_compat_query_system_theme_from_io_still_returns_theme() {
        // The OSC 11 background-only convenience used by other call
        // sites must keep working over the unified parser.
        let mut input = Cursor::new(b"\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?1;2c".to_vec());
        let mut output = Vec::new();
        let theme = query_system_theme_from_io(&mut input, &mut output);
        assert_eq!(theme, Some(SystemTheme::Light));
        assert_eq!(output, b"\x1b]10;?\x07\x1b]11;?\x07\x1b[c");
    }

    #[test]
    fn is_tty_is_false_under_cargo_test() {
        // `cargo test` runs with redirected stdio; the TTY gate must
        // evaluate to false there. This is the property we lean on to
        // skip the OSC round-trip in `agent --serve` and CI runs —
        // documenting it as a test makes regressions in `is_tty`
        // visible.
        assert!(!is_tty(), "expected non-tty under cargo test");
    }

    #[test]
    fn missing_osc_10_reply_returns_none_foreground_only() {
        // Some terminals reply to OSC 11 but not OSC 10. The batch
        // must succeed with a populated background and a None
        // foreground rather than failing both.
        let mut input = Cursor::new(b"\x1b]11;rgb:0000/0000/0000\x07\x1b[?1;2c".to_vec());
        let mut output = Vec::new();
        let (fg, bg) = query_terminal_colors_from_io(&mut input, &mut output).unwrap();
        assert_eq!(fg, None);
        assert_eq!(bg, rgb(0, 0, 0));
    }
}
