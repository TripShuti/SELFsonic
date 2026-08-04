//! Вкладка «Queue»: жива черга відтворення (у т.ч. черга, що будує DJ).
//! Поточний трек позначається `*`.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem as TuiListItem;
use ratatui::Frame;

use crate::playback::engine::Engine;

use super::super::app::{AppState, ListItem};
use super::super::theme;
use super::{fmt_duration, render_list_rows};

pub fn render(frame: &mut Frame, area: Rect, app: &AppState, engine: &Engine) {
    let current_id = engine.current().map(|t| t.id.clone());
    render_list_rows(frame, area, &app.list_title, &app.list, app.selected, |item| {
        queue_row(item, &current_id)
    });
}

fn queue_row(item: &ListItem, current_id: &Option<String>) -> TuiListItem<'static> {
    let ListItem::Track(t) = item else {
        return TuiListItem::new(Line::raw(""));
    };
    let is_current = Some(&t.id) == current_id.as_ref();
    let style = if is_current { theme::green() } else { theme::fg() };
    let mut spans = vec![
        Span::styled(if is_current { "* " } else { "  " }, style),
        Span::styled(t.title.clone(), style),
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
    TuiListItem::new(Line::from(spans))
}