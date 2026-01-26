use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

use crate::app::{App, View};
use crate::ui::widgets::{
    BusyModalWidget, ConfirmDialogWidget, EmailListWidget, GroupListWidget, HelpBarWidget,
    ThreadViewWidget, UiState,
};

/// Renders the entire application UI
pub fn render(frame: &mut Frame, app: &App, ui_state: &UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Main content
            Constraint::Length(1), // Help bar
        ])
        .split(frame.area());

    // Render main content based on view
    match app.view {
        View::GroupList => {
            let widget = GroupListWidget::new(app);
            frame.render_widget(widget, chunks[0]);
        }
        View::EmailList => {
            let widget = EmailListWidget::new(app);
            frame.render_widget(widget, chunks[0]);
        }
        View::Thread => {
            let widget = ThreadViewWidget::new(app);
            frame.render_widget(widget, chunks[0]);
        }
    }

    // Render help bar
    let help = HelpBarWidget::new(app);
    frame.render_widget(help, chunks[1]);

    // Render confirmation dialog if active
    if let Some(action) = &ui_state.confirm_action {
        let dialog_area = centered_rect(60, 30, frame.area());
        let dialog = ConfirmDialogWidget::new(action);
        frame.render_widget(dialog, dialog_area);
    }

    // Render busy modal if active (takes priority over confirmation)
    if ui_state.is_busy()
        && let Some(msg) = &ui_state.status_message
    {
        let modal = BusyModalWidget::new(msg, ui_state.spinner_char());
        frame.render_widget(modal, frame.area());
    }
}

/// Creates a centered rectangle for dialogs
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 100);
        let centered = centered_rect(50, 50, area);

        // Should be roughly centered
        assert!(centered.x > 0);
        assert!(centered.y > 0);
        assert!(centered.width < area.width);
        assert!(centered.height < area.height);
    }
}
