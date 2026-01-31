use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, TableState},
};

use crate::app::{App, View};
use crate::ui::widgets::{
    AccountSelectWidget, AccountSelection, BusyModalWidget, ConfirmDialogWidget, EmailListWidget,
    GroupListWidget, HelpBarWidget, HelpMenuWidget, InboxZeroWidget, SearchBarWidget,
    StatusModalWidget, TextViewWidget, ThreadViewWidget, UiState, UndoHistoryWidget,
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
            // Check if inbox is empty - show celebration screen!
            // Only show if emails have been loaded (empty before loading is not inbox zero)
            if app.groups.is_empty() && app.has_loaded_emails() {
                ui_state.tick_celebration();
                let widget = InboxZeroWidget::new(ui_state.celebration_frame);
                frame.render_widget(widget, chunks[0]);
            } else {
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
        View::UndoHistory => {
            // Render the previous view as background
            match app.previous_view() {
                Some(View::EmailList) => {
                    ui_state.viewport_heights.email_list = inner_height;
                    let widget = EmailListWidget::new(app);
                    let mut table_state = TableState::default().with_selected(app.selected_email);
                    frame.render_stateful_widget(widget, chunks[0], &mut table_state);
                }
                Some(View::Thread) => {
                    ui_state.viewport_heights.thread_view = inner_height;
                    let widget = ThreadViewWidget::new(app);
                    let mut table_state =
                        TableState::default().with_selected(app.selected_thread_email);
                    frame.render_stateful_widget(widget, chunks[0], &mut table_state);
                }
                _ => {
                    // Default to group list for GroupList or None
                    ui_state.viewport_heights.group_list = inner_height;
                    let widget = GroupListWidget::new(app, ui_state.group_scroll_offset);
                    frame.render_widget(widget, chunks[0]);
                }
            }

            // Calculate modal height for viewport tracking
            let modal_height = (inner_height as f32 * 0.6) as usize;
            ui_state.viewport_heights.undo_history = modal_height.saturating_sub(2); // Account for borders

            // Calculate scroll offset to keep selection visible
            let selected = app.selected_undo;
            let height = ui_state.viewport_heights.undo_history;
            let offset = &mut ui_state.undo_scroll_offset;

            if selected < *offset {
                *offset = selected;
            } else if selected >= *offset + height && height > 0 {
                *offset = selected.saturating_sub(height) + 1;
            }

            // Render undo history modal on top
            let widget = UndoHistoryWidget::new(app, *offset);
            frame.render_widget(widget, chunks[0]);
        }
        View::EmailBody => {
            ui_state.viewport_heights.text_view = inner_height;

            // Clamp scroll position to valid range
            // (we don't know exact line count but prevent going too far)
            let scroll = app.text_view_scroll.min(10000);

            let widget = TextViewWidget::new(app, scroll, &ui_state.text_view_state);
            frame.render_widget(widget, chunks[0]);
        }
    }

    // Render help bar or search bar
    if ui_state.is_searching() {
        let search = SearchBarWidget::new(ui_state.search_query());
        frame.render_widget(search, chunks[1]);
    } else {
        let help = HelpBarWidget::new(app);
        frame.render_widget(help, chunks[1]);
    }

    // Render confirmation dialog if active
    if let Some(action) = &ui_state.confirm_action {
        let dialog = ConfirmDialogWidget::new(action);
        frame.render_widget(dialog, frame.area());
    }

    // Render help menu if active
    if ui_state.is_showing_help() {
        let help_menu = HelpMenuWidget::new(app.view);
        frame.render_widget(help_menu, frame.area());
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
