//! Keybindings → actions (vim style, AGENT.md).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    Bottom,
    Select,
    Back,
    NextTab,
    PrevTab,
    PlayPause,
    Next,
    Previous,
    CycleLoop,
    ToggleShuffle,
    VolumeUp,
    VolumeDown,
    SeekForward,
    SeekBackward,
    Refresh,
    DJ,
    Quit,
}

impl Action {
    /// Action for normal view mode.
    pub fn from_key(key: KeyEvent) -> Option<Action> {
        let code = key.code;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Char('q') if ctrl => Some(Action::Quit),
            KeyCode::Char('c') if ctrl => Some(Action::Quit),
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Esc => Some(Action::Back),
            KeyCode::Enter => Some(Action::Select),
            KeyCode::Down | KeyCode::Char('j') => Some(Action::Down),
            KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
            KeyCode::PageDown | KeyCode::Char('d') if ctrl => Some(Action::PageDown),
            KeyCode::PageUp | KeyCode::Char('u') if ctrl => Some(Action::PageUp),
            KeyCode::Home | KeyCode::Char('g') => Some(Action::Top),
            KeyCode::End | KeyCode::Char('G') => Some(Action::Bottom),
            KeyCode::Char(' ') => Some(Action::PlayPause),
            KeyCode::Char('n') => Some(Action::Next),
            KeyCode::Char('p') => Some(Action::Previous),
            KeyCode::Char('l') => Some(Action::CycleLoop),
            KeyCode::Char('s') => Some(Action::ToggleShuffle),
            KeyCode::Char('+') | KeyCode::Char('=') => Some(Action::VolumeUp),
            KeyCode::Char('-') => Some(Action::VolumeDown),
            KeyCode::Char(']') => Some(Action::SeekForward),
            KeyCode::Char('[') => Some(Action::SeekBackward),
            KeyCode::Char('r') => Some(Action::Refresh),
            KeyCode::Char('d') => Some(Action::DJ),
            KeyCode::Tab => Some(Action::NextTab),
            KeyCode::BackTab => Some(Action::PrevTab),
            KeyCode::Right => Some(Action::NextTab),
            KeyCode::Left => Some(Action::PrevTab),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn vim_navigation() {
        assert_eq!(Action::from_key(key(KeyCode::Char('j'))), Some(Action::Down));
        assert_eq!(Action::from_key(key(KeyCode::Char('k'))), Some(Action::Up));
        assert_eq!(Action::from_key(key(KeyCode::Char('g'))), Some(Action::Top));
        assert_eq!(Action::from_key(key(KeyCode::Char('G'))), Some(Action::Bottom));
        assert_eq!(Action::from_key(key(KeyCode::Char(' '))), Some(Action::PlayPause));
        assert_eq!(Action::from_key(key(KeyCode::Char('d'))), Some(Action::DJ));
        // Ctrl+d — це PageDown, а не DJ.
        assert_eq!(
            Action::from_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(Action::PageDown)
        );
    }

    #[test]
    fn special_keys() {
        assert_eq!(Action::from_key(key(KeyCode::Enter)), Some(Action::Select));
        assert_eq!(Action::from_key(key(KeyCode::Esc)), Some(Action::Back));
        assert_eq!(Action::from_key(key(KeyCode::Tab)), Some(Action::NextTab));
    }
}
