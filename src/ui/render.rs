use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders, TableState},
};

use crate::app::{App, View};
use crate::ui::widgets::{
    AccountSelectWidget, AccountSelection, BusyModalWidget, ConfirmDialogWidget, EmailListWidget,
    GroupListWidget, HelpBarWidget, StatusModalWidget, ThreadViewWidget, UiState,
};

/// Renders the entire application UI
pub fn render(frame: &mut Frame, app: &App, ui_state: &mut UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Main content
            Constraint::Length(1), // Help bar
        ])
        .split(frame.area());

    // Calculate inner height for viewport tracking (account for borders)
    let block = Block::default().borders(Borders::ALL);
    let inner_height = block.inner(chunks[0]).height as usize;

    // Render main content based on view
    match app.view {
        View::GroupList => {
            ui_state.viewport_heights.group_list = inner_height;

            // Calculate scroll offset to keep selection visible
            let selected = app.selected_group;
            let height = inner_height;
            let offset = &mut ui_state.group_scroll_offset;

            if selected < *offset {
                *offset = selected;
            } else if selected >= *offset + height {
                *offset = selected.saturating_sub(height) + 1;
            }

            let widget = GroupListWidget::new(app, *offset);
            frame.render_widget(widget, chunks[0]);
        }
        View::EmailList => {
            ui_state.viewport_heights.email_list = inner_height;

            let widget = EmailListWidget::new(app);
            let mut table_state = TableState::default().with_selected(app.selected_email);
            frame.render_stateful_widget(widget, chunks[0], &mut table_state);
        }
        View::Thread => {
            ui_state.viewport_heights.thread_view = inner_height;

            let widget = ThreadViewWidget::new(app);
            let mut table_state = TableState::default().with_selected(app.selected_thread_email);
            frame.render_stateful_widget(widget, chunks[0], &mut table_state);
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

    // Render status modal for non-busy messages (warnings, errors)
    if !ui_state.is_busy()
        && !ui_state.is_confirming()
        && let Some(msg) = &ui_state.status_message
    {
        let modal = StatusModalWidget::new(msg);
        frame.render_widget(modal, frame.area());
    }
}

/// Renders the account selection UI
pub fn render_account_select(frame: &mut Frame, selection: &AccountSelection) {
    let widget = AccountSelectWidget::new(selection);
    frame.render_widget(widget, frame.area());
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
