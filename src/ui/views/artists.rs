//! Вкладка «Артисти»: список з кешу.

use ratatui::layout::Rect;
use ratatui::Frame;

use super::super::app::{AppState, ListItem};
use super::super::theme;
use super::render_list;

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    if app.list.is_empty() && !app.loading {
        render_empty(frame, area, "Library is empty — press r to refresh");
        return;
    }
    render_list(frame, area, &app.list_title, &app.list, app.selected);
}

fn render_empty(frame: &mut Frame, area: Rect, msg: &str) {
    let block = ratatui::widgets::Block::bordered()
        .title("Artists")
        .border_style(theme::border());
    let text = ratatui::text::Text::styled(msg, theme::fg_dim());
    let paragraph = ratatui::widgets::Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

#[allow(dead_code)]
fn _keep(item: &ListItem) {
    let _ = item;
}
