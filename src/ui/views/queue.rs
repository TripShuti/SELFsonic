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
    let starred = &app.starred_ids;
    let anchor = current_id.as_ref().and_then(|id| {
        app.list
            .iter()
            .position(|item| matches!(item, ListItem::Track(t) if &t.id == id))
    });
    render_list_rows(
        frame,
        area,
        &app.list_title,
        &app.list,
        app.selected,
        anchor,
        |item| queue_row(item, &current_id, starred),
    );
}

fn queue_row(
    item: &ListItem,
    current_id: &Option<String>,
    starred: &std::collections::HashSet<String>,
) -> TuiListItem<'static> {
    let ListItem::Track(t) = item else {
        return TuiListItem::new(Line::raw(""));
    };
    let is_current = Some(&t.id) == current_id.as_ref();
    let is_starred = starred.contains(&t.id);
    let style = if is_current { theme::green() } else { theme::fg() };
    let mut spans = vec![
        Span::styled(if is_current { "* " } else { "  " }, style),
        Span::styled(
            if is_starred {
                format!("{} ", super::HEART)
            } else {
                "  ".to_string()
            },
            theme::red(),
        ),
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