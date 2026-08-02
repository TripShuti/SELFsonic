//! Now playing status bar (always visible).

use std::time::Duration;

use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::playback::engine::{Engine, LoopMode};

use super::super::app::AppState;
use super::super::theme;
use super::{fmt_duration, fmt_volume};

pub fn render(frame: &mut Frame, area: Rect, engine: &Engine, app: &AppState) {
    let height = area.height.saturating_sub(1);
    let bar = Block::bordered().border_style(theme::FG_DIM);
    let inner = bar.inner(area);

    let mut lines: Vec<Line> = Vec::new();

    // Error/status line, if any.
    if let Some(msg) = &app.message {
        let style = if msg.is_error { theme::RED } else { theme::GREEN };
        lines.push(Line::from(Span::styled(msg.text.clone(), style)));
    } else if app.loading {
        lines.push(Line::from(Span::styled("Loading...", theme::YELLOW)));
    }

    match engine.current() {
        Some(track) => {
            let playing = !engine.paused() && !engine.stopped();
            let icon = if playing { ">" } else { "||" };
            let title = format!("{}  {} — {}", icon, track.artist, track.title);
            let pos = engine.position();
            let total = Duration::from_secs(track.duration.max(0) as u64);

            let line = if height == 1 {
                Line::from(vec![Span::raw(title)])
            } else {
                // 01:23 [███████░░░░] 04:05  42%
                let ratio = if total.is_zero() {
                    0.0
                } else {
                    (pos.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0)
                };
                // time + separators + brackets + pct
                let fixed = 5 + 2 + 2 + 5 + 2 + 3;
                let bar_width = (inner.width as usize).saturating_sub(fixed).max(10);
                let filled = (bar_width as f64 * ratio).round() as usize;
                let spans = vec![
                    Span::styled(fmt_duration(pos.as_secs() as i32), theme::FG_DIM),
                    Span::raw("  "),
                    Span::styled("[", theme::FG_DIM),
                    Span::styled("█".repeat(filled), theme::GREEN),
                    Span::styled("░".repeat(bar_width.saturating_sub(filled)), theme::FG_DIM),
                    Span::styled("]", theme::FG_DIM),
                    Span::raw("  "),
                    Span::styled(fmt_duration(track.duration), theme::FG_DIM),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:3}%", (ratio * 100.0).round() as u32),
                        theme::YELLOW,
                    ),
                ];
                Line::from(spans)
            };
            lines.push(line);

            let controls = format!(
                "{}  ·  {}  ·  {}",
                loop_label(&engine.loop_mode()),
                if engine.shuffle() { "shuffle: on" } else { "shuffle: off" },
                fmt_volume(engine.volume()),
            );
            lines.push(Line::from(Span::styled(controls, theme::FG_DIM)));
        }
        None => {
            lines.push(Line::from(Span::styled(
                "Nothing playing — press Enter on an album or track.  (r — refresh library, q — quit)",
                theme::FG_DIM,
            )));
            if height > 1 {
                lines.push(Line::from(Span::styled(
                    "space pause  n/p next/prev  l repeat  s shuffle  +/- volume  [ / ] seek",
                    theme::FG_DIM,
                )));
            }
        }
    }

    let paragraph = Paragraph::new(lines).block(bar).alignment(Alignment::Left);
    frame.render_widget(paragraph, area);
}

fn loop_label(mode: &LoopMode) -> &'static str {
    match mode {
        LoopMode::None => "repeat: off",
        LoopMode::Track => "repeat: 1",
        LoopMode::Playlist => "repeat: all",
    }
}
