//! Палітра gruvbox (dark), консистентно з іншими TUI-проєктами (AGENT.md).

use ratatui::style::{Color, Style};

pub const BG: Color = Color::Rgb(0x28, 0x28, 0x28);
pub const BG_SELECTED: Color = Color::Rgb(0x50, 0x49, 0x45);
pub const FG: Color = Color::Rgb(0xeb, 0xdb, 0xb2);
pub const FG_DIM: Color = Color::Rgb(0xa8, 0x99, 0x84);
pub const RED: Color = Color::Rgb(0xcc, 0x24, 0x1d);
pub const GREEN: Color = Color::Rgb(0x98, 0x97, 0x1a);
pub const YELLOW: Color = Color::Rgb(0xd7, 0x99, 0x21);
pub const ORANGE: Color = Color::Rgb(0xd6, 0x5d, 0x0e);

pub fn base() -> Style {
    Style::new().fg(FG).bg(BG)
}

pub fn selected() -> Style {
    Style::new().fg(FG).bg(BG_SELECTED)
}

pub fn title(tab_active: bool) -> Style {
    if tab_active {
        Style::new().fg(BG).bg(ORANGE).add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        Style::new().fg(FG_DIM)
    }
}
