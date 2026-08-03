//! Адаптивна тема: фон/текст — термінальні (`Reset`), акценти — кольори
//! 16-кольорової палітри терміналу (OSC 4 запит на старті через terminal-trx).
//! Fallback — яскраві truecolor набори за темним/світлим фоном (OSC 10/11).

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use terminal_colorsaurus::{QueryOptions, ThemeMode};

pub struct Theme {
    pub fg: Color,
    pub fg_dim: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub border: Color,
    accent: Color,
    selection_bg: Color,
}

impl Theme {
    fn dark() -> Self {
        Self {
            fg: Color::Reset,
            fg_dim: Color::Rgb(0x9e, 0x9e, 0x9e),
            red: Color::Rgb(0xf8, 0x51, 0x49),
            green: Color::Rgb(0x3f, 0xb9, 0x50),
            yellow: Color::Rgb(0xd2, 0x99, 0x22),
            border: Color::Rgb(0x7e, 0x77, 0x99),
            accent: Color::Rgb(0xe8, 0x86, 0x2e),
            selection_bg: Color::Rgb(0x4a, 0x4a, 0x55),
        }
    }

    fn light() -> Self {
        Self {
            fg: Color::Reset,
            fg_dim: Color::Rgb(0x61, 0x61, 0x61),
            red: Color::Rgb(0xc6, 0x28, 0x28),
            green: Color::Rgb(0x2e, 0x7d, 0x32),
            yellow: Color::Rgb(0xa0, 0x6c, 0x00),
            border: Color::Rgb(0x5f, 0x58, 0x75),
            accent: Color::Rgb(0xb4, 0x5f, 0x06),
            selection_bg: Color::Rgb(0xd6, 0xd3, 0xde),
        }
    }

    /// Тема зі справжньої палітри терміналу; невідомі слоти — з fallback-набору.
    fn from_palette(palette: &Palette, mode: ThemeMode) -> Self {
        let fallback = match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        };
        Self {
            fg: Color::Reset,
            fg_dim: palette.color8.map(rgb).unwrap_or(fallback.fg_dim),
            red: palette.color1.map(rgb).unwrap_or(fallback.red),
            green: palette.color2.map(rgb).unwrap_or(fallback.green),
            yellow: palette.color3.map(rgb).unwrap_or(fallback.yellow),
            border: palette
                .color5
                .map(rgb)
                .or_else(|| palette.color3.map(rgb))
                .unwrap_or(fallback.border),
            accent: palette
                .color3
                .map(rgb)
                .or_else(|| palette.color2.map(rgb))
                .unwrap_or(fallback.accent),
            selection_bg: fallback.selection_bg,
        }
    }
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

static THEME: OnceLock<Theme> = OnceLock::new();

/// Визначити палітру/фон терміналу і зафіксувати тему.
pub fn init() {
    let theme = match query_colors() {
        Some(palette) => {
            let mode = palette
                .theme_mode()
                .or_else(|| detect_theme_mode().ok())
                .unwrap_or(ThemeMode::Dark);
            Theme::from_palette(&palette, mode)
        }
        None => match detect_theme_mode() {
            Ok(ThemeMode::Light) => Theme::light(),
            _ => Theme::dark(),
        },
    };
    let _ = THEME.set(theme);
}

fn detect_theme_mode() -> Result<ThemeMode, ()> {
    let mut options = QueryOptions::default();
    options.timeout = Duration::from_millis(250);
    terminal_colorsaurus::theme_mode(options).map_err(|_| ())
}

fn current() -> &'static Theme {
    THEME.get_or_init(Theme::dark)
}

pub fn fg() -> Color {
    current().fg
}

pub fn fg_dim() -> Color {
    current().fg_dim
}

pub fn red() -> Color {
    current().red
}

pub fn green() -> Color {
    current().green
}

pub fn yellow() -> Color {
    current().yellow
}

pub fn border() -> Color {
    current().border
}

pub fn base() -> Style {
    Style::default()
}

pub fn selected() -> Style {
    Style::new().bg(current().selection_bg)
}

pub fn title(tab_active: bool) -> Style {
    let t = current();
    if tab_active {
        Style::new()
            .fg(Color::Rgb(0x1f, 0x1f, 0x1f))
            .bg(t.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(t.fg_dim)
    }
}

// ---------- OSC 4/10/11 запит ----------

#[derive(Default)]
struct Palette {
    color1: Option<(u8, u8, u8)>,
    color2: Option<(u8, u8, u8)>,
    color3: Option<(u8, u8, u8)>,
    color5: Option<(u8, u8, u8)>,
    color8: Option<(u8, u8, u8)>,
    fg: Option<(u8, u8, u8)>,
    bg: Option<(u8, u8, u8)>,
}

impl Palette {
    fn theme_mode(&self) -> Option<ThemeMode> {
        let fg = self.fg?;
        let bg = self.bg?;
        let fg_l = lightness(fg);
        let bg_l = lightness(bg);
        if bg_l < fg_l {
            Some(ThemeMode::Dark)
        } else if bg_l > fg_l || bg_l > 0.5 {
            Some(ThemeMode::Light)
        } else {
            Some(ThemeMode::Dark)
        }
    }
}

fn lightness((r, g, b): (u8, u8, u8)) -> f32 {
    terminal_colorsaurus::Color::rgb(r as u16, g as u16, b as u16).perceived_lightness()
}

/// Один burst-запит: 16-палітра (слоти 1,2,3,5,8) + текст (10) + фон (11).
fn query_colors() -> Option<Palette> {
    let mut term = terminal_trx::terminal().ok()?;
    let mut lock = term.lock();
    let mut tty = lock.enable_raw_mode().ok()?;
    tty.write_all(
        b"\x1b]4;1;?;2;?;3;?;5;?;8;?\x1b\\\x1b]10;?\x1b\\\x1b]11;?\x1b\\",
    )
    .ok()?;
    tty.flush().ok()?;

    let deadline = Instant::now() + Duration::from_millis(250);
    let mut buf = Vec::with_capacity(512);
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let ms = deadline.saturating_duration_since(now).as_millis().min(250) as i32;
        let mut pfd = libc::pollfd {
            fd: tty.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
        if rc <= 0 || pfd.revents & libc::POLLIN == 0 {
            break;
        }
        let mut chunk = [0u8; 1024];
        let n = tty.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(2).filter(|w| *w == b"\x1b]").count() >= 7 {
            break;
        }
    }
    drop(tty);
    Some(parse_response(&buf))
}

fn parse_response(buf: &[u8]) -> Palette {
    let mut palette = Palette::default();
    let mut rest = buf;
    while let Some(rel) = find_pattern(rest, b"\x1b]") {
        let start = rel + 2;
        let (end, consumed) = response_end(&rest[start..]);
        parse_payload(&rest[start..start + end], &mut palette);
        rest = &rest[start + consumed..];
    }
    palette
}

/// Кінець payload OSC-відповіді і скільки байтів спожито: термінатор —
/// ST (`\x1b\\`) або BEL (`\x07`); нова послідовність (`\x1b]`/`\x1b[`)
/// завершує поточну без споживання.
fn response_end(rest: &[u8]) -> (usize, usize) {
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            b'\x07' => return (i, i + 1),
            b'\x1b' if rest.get(i + 1) == Some(&b'\\') => return (i, i + 2),
            b'\x1b' if matches!(rest.get(i + 1), Some(&b']') | Some(&b'[')) => return (i, i),
            b'\x1b' => i += 1,
            _ => i += 1,
        }
    }
    (rest.len(), rest.len())
}

fn find_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_payload(payload: &[u8], palette: &mut Palette) {
    let text = match std::str::from_utf8(payload) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut parts = text.split(';');
    let code = parts.next().unwrap_or("");
    let rest = parts.collect::<Vec<_>>().join(";");
    match code {
        "10" => palette.fg = parse_rgb(&rest),
        "11" => palette.bg = parse_rgb(&rest),
        "4" => {
            let mut it = rest.splitn(2, ';');
            let idx: u8 = match it.next().and_then(|s| s.parse().ok()) {
                Some(i) => i,
                None => return,
            };
            let color = parse_rgb(it.next().unwrap_or(""));
            let slot = match idx {
                1 => &mut palette.color1,
                2 => &mut palette.color2,
                3 => &mut palette.color3,
                5 => &mut palette.color5,
                8 => &mut palette.color8,
                _ => return,
            };
            *slot = color;
        }
        _ => {}
    }
}

fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.strip_prefix("rgb:")?;
    let mut parts = hex.split('/');
    let r = parse_hex_comp(parts.next()?)?;
    let g = parse_hex_comp(parts.next()?)?;
    let b = parse_hex_comp(parts.next()?)?;
    Some((r, g, b))
}

/// `rrrr` (4-digit) | `rrr` | `rr` | `r` — беремо перші два hex-знаки.
fn parse_hex_comp(comp: &str) -> Option<u8> {
    let s = &comp[..comp.len().min(2)];
    u8::from_str_radix(s, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_st_terminated_responses() {
        let buf = b"\x1b]4;1;rgb:ffff/b4b4/abab\x1b\\\x1b]4;8;rgb:cbcb/c4c4/cfcf\x1b\\\x1b]11;rgb:3232/2f2f/3333\x1b\\";
        let p = parse_response(buf);
        assert_eq!(p.color1, Some((0xff, 0xb4, 0xab)));
        assert_eq!(p.color8, Some((0xcb, 0xc4, 0xcf)));
        assert_eq!(p.bg, Some((0x32, 0x2f, 0x33)));    }

    #[test]
    fn parses_bel_terminated_responses() {
        let buf = b"\x1b]4;3;rgb:f1f1/b7b7/c2c2\x07\x1b]10;rgb:e7e7/e0e0/e8e8\x07";
        let p = parse_response(buf);
        assert_eq!(p.color3, Some((0xf1, 0xb7, 0xc2)));
        assert_eq!(p.fg, Some((0xe7, 0xe0, 0xe8)));
    }

    #[test]
    fn ignores_garbage_between_responses() {
        let buf = b"\x1b[?1;2c\x1b]4;5;rgb:b2b2/a7a7/bfbf\x1b\\";
        let p = parse_response(buf);
        assert_eq!(p.color5, Some((0xb2, 0xa7, 0xbf)));
    }
}
