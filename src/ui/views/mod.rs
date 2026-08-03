//! Рендер в'юх: список у головній панелі + статус-бар.

pub mod albums;
pub mod artists;
pub mod now_playing;
pub mod tracks;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem as TuiListItem, ListState};
use ratatui::Frame;

use super::app::{ListItem, list_row_style};
use super::theme;

pub fn row_items(items: &[ListItem]) -> Vec<TuiListItem<'static>> {
    items.iter().map(row_item).collect()
}

fn row_item(item: &ListItem) -> TuiListItem<'static> {
    let line = match item {
        ListItem::Artist { name, album_count, .. } => Line::from(vec![
            Span::raw(name.clone()),
            Span::styled(
                format!("  · {} alb.", album_count.unwrap_or(0)),
                theme::fg_dim(),
            ),
        ]),
        ListItem::Album { name, artist, year, duration, .. } => {
            let mut spans = vec![
                Span::styled(name.clone(), theme::fg()),
                Span::styled(format!("  — {artist}"), theme::fg_dim()),
            ];
            let right = format!(
                "{}{}",
                year.map(|y| format!("  {y}")).unwrap_or_default(),
                if *duration > 0 {
                    format!("  {}", fmt_duration(*duration))
                } else {
                    String::new()
                }
            );
            spans.push(Span::styled(right, theme::fg_dim()));
            Line::from(spans)
        }
        ListItem::Track(t) => {
            let num = t
                .track_number
                .map(|n| format!("{n:>2}. "))
                .unwrap_or_default();
            let mut spans = vec![
                Span::styled(num, theme::fg_dim()),
                Span::styled(t.title.clone(), theme::fg()),
            ];
            if !t.artist.is_empty() {
                spans.push(Span::styled(format!("  — {}", t.artist), theme::fg_dim()));
            }
            if t.duration > 0 {
                spans.push(Span::styled(
                    format!("  {}", fmt_duration(t.duration)),
                    theme::fg_dim(),
                ));
            }
            Line::from(spans)
        }
        ListItem::Playlist { name, song_count, .. } => Line::from(vec![
            Span::raw(name.clone()),
            Span::styled(format!("  · {song_count} tracks"), theme::fg_dim()),
        ]),
        ListItem::More => Line::styled(
            "--- load more ---",
            Style::default().fg(theme::fg_dim()).italic(),
        ),
    };
    TuiListItem::new(line)
}

pub fn fmt_duration(secs: i32) -> String {
    let s = secs.max(0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: &[ListItem],
    selected: usize,
) {
    let list = List::new(row_items(items))
        .block(Block::bordered().title(title).border_style(theme::border()))
        .highlight_style(list_row_style(true))
        .highlight_symbol("> ")
        .highlight_spacing(ratatui::widgets::HighlightSpacing::Always);

    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(selected.min(items.len() - 1)));
    }
    // Тримаємо вибір у межах видимої області.
    let visible = area.height.saturating_sub(2) as usize;
    let offset = state.selected().unwrap_or(0);
    if offset >= visible {
        *state.offset_mut() = offset.saturating_sub(visible.saturating_sub(1));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

pub fn fmt_volume(v: f32) -> String {
    format!("Volume: {:3}%", (v.clamp(0.0, 1.0) * 100.0).round() as u32)
}
