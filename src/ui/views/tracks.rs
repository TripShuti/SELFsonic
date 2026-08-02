//! Рівень треків (альбом/плейлист/пошук).

use ratatui::layout::Rect;
use ratatui::Frame;

use super::super::app::AppState;
use super::render_list;

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    render_list(frame, area, &app.list_title, &app.list, app.selected);
}
