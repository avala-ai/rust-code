//! Terminal query helpers for theme auto-detection.

use std::io::{self, IsTerminal, Read, Write};
use std::sync::OnceLock;
use std::time::Duration;

#[cfg(unix)]
use crossterm::terminal;

pub const SYSTEM_THEME_ENV: &str = "AGENT_CODE_SYSTEM_THEME";

const OSC_11_QUERY: &[u8] = b"\x1b]11;?\x07";
const DA1_QUERY: &[u8] = b"\x1bc";
const QUERY_TIMEOUT: Duration = Duration::from_secs(1);

static SYSTEM_THEME: OnceLock<SystemTheme> = OnceLock::new();

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
    let theme = *SYSTEM_THEME.get_or_init(detect_system_theme_uncached);
    prime_system_theme_env(theme);
    theme
}

fn prime_system_theme_env(theme: SystemTheme) {
    if std::env::var(SYSTEM_THEME_ENV)
        .ok()
        .and_then(|value| SystemTheme::from_env(&value))
        .is_some()
    {
        return;
    }

    // SAFETY: this is called during theme initialization before the agent
    // starts spawning child processes. The env value lets those children reuse
    // the parent's result instead of re-querying the terminal.
    unsafe { std::env::set_var(SYSTEM_THEME_ENV, theme.as_str()) };
}

fn detect_system_theme_uncached() -> SystemTheme {
    if let Ok(value) = std::env::var(SYSTEM_THEME_ENV)
        && let Some(theme) = SystemTheme::from_env(&value)
    {
        return theme;
    }

    if let Some(theme) = colorfgbg_theme() {
        return theme;
    }

    if std::env::var("TERM_PROGRAM")
        .ok()
        .is_some_and(|p| p == "Apple_Terminal")
    {
        return SystemTheme::Light;
    }

    query_terminal_theme().unwrap_or(SystemTheme::Dark)
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

#[cfg(unix)]
fn query_terminal_theme() -> Option<SystemTheme> {
    if !is_tty() {
        return None;
    }

    let raw_was_enabled = terminal::is_raw_mode_enabled().unwrap_or(false);
    if !raw_was_enabled && terminal::enable_raw_mode().is_err() {
        return None;
    }

    let result = (|| {
        let mut stdout = io::stdout();
        stdout.write_all(OSC_11_QUERY).ok()?;
        stdout.write_all(DA1_QUERY).ok()?;
        stdout.flush().ok()?;

        let bytes = read_stdin_until_da1(QUERY_TIMEOUT)?;
        parse_background_color(&bytes).map(theme_for_rgb)
    })();

    if !raw_was_enabled {
        let _ = terminal::disable_raw_mode();
    }

    result
}

#[cfg(not(unix))]
fn query_terminal_theme() -> Option<SystemTheme> {
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
            if err.kind() == io::ErrorKind::Interrupted
                || err.kind() == io::ErrorKind::WouldBlock
            {
                continue;
            }
            return None;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
}

pub fn query_system_theme_from_io<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> Option<SystemTheme> {
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

    parse_background_color(&out).map(theme_for_rgb)
}

pub fn parse_background_color(input: &[u8]) -> Option<Rgb> {
    let text = std::str::from_utf8(input).ok()?;
    let mut rest = text;
    while let Some(idx) = rest.find("\x1b]11;") {
        let payload_start = idx + "\x1b]11;".len();
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
    let component_len = hex.len().checked_div(3)?;
    if component_len == 0 || component_len > 4 || component_len * 3 != hex.len() {
        return None;
    }
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

    #[test]
    fn parses_rgb_components_with_one_to_four_digits() {
        assert_eq!(
            parse_background_color(&osc("rgb:f/0/8")),
            Some(Rgb {
                r: 255,
                g: 0,
                b: 136
            })
        );
        assert_eq!(
            parse_background_color(&osc("rgb:80/40/20")),
            Some(Rgb {
                r: 128,
                g: 64,
                b: 32
            })
        );
        assert_eq!(
            parse_background_color(&osc("rgb:800/400/200")),
            Some(Rgb {
                r: 128,
                g: 64,
                b: 32
            })
        );
        assert_eq!(
            parse_background_color(&osc("rgb:8000/4000/2000")),
            Some(Rgb {
                r: 128,
                g: 64,
                b: 32
            })
        );
    }

    #[test]
    fn parses_rgba_and_ignores_alpha() {
        assert_eq!(
            parse_background_color(&osc("rgba:ffff/8000/0000/ffff")),
            Some(Rgb {
                r: 255,
                g: 128,
                b: 0
            })
        );
    }

    #[test]
    fn parses_hash_forms() {
        assert_eq!(
            parse_background_color(&osc("#fff")),
            Some(Rgb {
                r: 255,
                g: 255,
                b: 255
            })
        );
        assert_eq!(
            parse_background_color(&osc("#804020")),
            Some(Rgb {
                r: 128,
                g: 64,
                b: 32
            })
        );
        assert_eq!(
            parse_background_color(&osc("#800400200")),
            Some(Rgb {
                r: 128,
                g: 64,
                b: 32
            })
        );
        assert_eq!(
            parse_background_color(&osc("#800040002000")),
            Some(Rgb {
                r: 128,
                g: 64,
                b: 32
            })
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
            Some(Rgb {
                r: 0,
                g: 255,
                b: 0
            })
        );
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
        let mut input = Cursor::new(b"\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?1;2cignored");
        let mut output = Vec::new();

        let theme = query_system_theme_from_io(&mut input, &mut output);

        assert_eq!(theme, Some(SystemTheme::Light));
        assert_eq!(output, b"\x1b]11;?\x07\x1bc");
        assert_eq!(input.position(), 32);
    }
}
