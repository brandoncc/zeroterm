use chrono::{DateTime, Datelike, Local, Utc};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, StatefulWidget, Table, TableState, Widget},
};

use crate::app::{App, GroupMode, UndoActionType, UndoContext, View};
use crate::config::AccountConfig;

/// Warning indicator character for messages
pub const WARNING_CHAR: char = '⚠';

/// Format a date for display in email lists
/// Shows time for current year, year for older emails
fn format_date(date: &DateTime<Utc>) -> String {
    let local: DateTime<Local> = date.with_timezone(&Local);
    let now = Local::now();

    if local.year() == now.year() {
        // Current year: "Jan 15 10:30"
        local.format("%b %d %H:%M").to_string()
    } else {
        // Previous years: "Jan 15  2024"
        local.format("%b %d  %Y").to_string()
    }
}

/// State for the confirmation dialog
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    /// Archive emails from a sender (only their emails, not full threads)
    ArchiveEmails { sender: String, count: usize },
    /// Delete emails from a sender (only their emails, not full threads)
    DeleteEmails { sender: String, count: usize },
    /// Archive entire thread (all emails including other senders)
    ArchiveThread { thread_email_count: usize },
    /// Delete entire thread (all emails including other senders)
    DeleteThread { thread_email_count: usize },
    /// Quit the application
    Quit,
}

impl ConfirmAction {
    pub fn message(&self) -> String {
        match self {
            ConfirmAction::ArchiveEmails { sender, count } => {
                format!("📥 Archive {} email(s) from {}? (y/n)", count, sender)
            }
            ConfirmAction::DeleteEmails { sender, count } => {
                format!("🗑  Delete {} email(s) from {}? (y/n)", count, sender)
            }
            ConfirmAction::ArchiveThread { thread_email_count } => {
                format!(
                    "📥 Archive entire thread ({} email(s))? (y/n)",
                    thread_email_count
                )
            }
            ConfirmAction::DeleteThread { thread_email_count } => {
                format!(
                    "🗑  Delete entire thread ({} email(s))? (y/n)",
                    thread_email_count
                )
            }
            ConfirmAction::Quit => "🚪 Quit zeroterm? (y/n)".to_string(),
        }
    }
}

/// Spinner frames for animated busy indicator
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Tracks viewport heights for each view to enable half-page scrolling
#[derive(Debug, Default, Clone, Copy)]
pub struct ViewportHeights {
    pub group_list: usize,
    pub email_list: usize,
    pub thread_view: usize,
    pub undo_history: usize,
}

impl ViewportHeights {
    /// Returns the viewport height for the given view
    pub fn for_view(&self, view: View) -> usize {
        match view {
            View::GroupList => self.group_list,
            View::EmailList => self.email_list,
            View::Thread => self.thread_view,
            View::UndoHistory => self.undo_history,
        }
    }
}

/// UI state that supplements App state
#[derive(Debug, Default)]
pub struct UiState {
    pub confirm_action: Option<ConfirmAction>,
    pub status_message: Option<String>,
    /// When true, the UI is busy with an IMAP operation and input is blocked
    pub busy: bool,
    /// Frame counter for spinner animation
    pub spinner_frame: usize,
    /// Viewport heights for half-page scrolling
    pub viewport_heights: ViewportHeights,
    /// Scroll offset for group list (manual scrolling since it uses custom rendering)
    pub group_scroll_offset: usize,
    /// Scroll offset for undo history list
    pub undo_scroll_offset: usize,
}

impl UiState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_confirm(&mut self, action: ConfirmAction) {
        self.confirm_action = Some(action);
    }

    pub fn clear_confirm(&mut self) {
        self.confirm_action = None;
    }

    pub fn is_confirming(&self) -> bool {
        self.confirm_action.is_some()
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
    }

    /// Set busy state with a status message (blocks input)
    pub fn set_busy(&mut self, msg: impl Into<String>) {
        self.busy = true;
        self.status_message = Some(msg.into());
        self.spinner_frame = 0;
    }

    /// Update the busy message without resetting the spinner
    pub fn update_busy_message(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
    }

    /// Clear busy state
    pub fn clear_busy(&mut self) {
        self.busy = false;
        self.status_message = None;
    }

    /// Returns true if the UI is busy and input should be blocked
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Advance the spinner animation frame
    pub fn tick_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
    }

    /// Get the current spinner character
    pub fn spinner_char(&self) -> char {
        SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()]
    }

    /// Clear the status message
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Returns true if there's a status message to display
    pub fn has_status(&self) -> bool {
        self.status_message.is_some()
    }
}

/// Widget for the busy/loading modal overlay
pub struct BusyModalWidget<'a> {
    message: &'a str,
    spinner: char,
}

impl<'a> BusyModalWidget<'a> {
    pub fn new(message: &'a str, spinner: char) -> Self {
        Self { message, spinner }
    }
}

impl Widget for BusyModalWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Format message with spinner
        let display_msg = format!("{} {}", self.spinner, self.message);

        // Calculate centered box size
        let msg_width = display_msg.len() as u16 + 4;
        let box_width = msg_width.max(20).min(area.width.saturating_sub(4));
        let box_height = 3;

        let x = area.x + (area.width.saturating_sub(box_width)) / 2;
        let y = area.y + (area.height.saturating_sub(box_height)) / 2;

        let modal_area = Rect::new(x, y, box_width, box_height);

        // Clear the area behind the modal
        for row in modal_area.y..modal_area.y + modal_area.height {
            for col in modal_area.x..modal_area.x + modal_area.width {
                buf[(col, row)].set_char(' ');
                buf[(col, row)].set_style(Style::default());
            }
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default());

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        // Center the message with spinner
        let msg_x = inner.x + (inner.width.saturating_sub(display_msg.len() as u16)) / 2;
        buf.set_line(
            msg_x,
            inner.y,
            &Line::from(Span::styled(
                display_msg,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            inner.width,
        );
    }
}

/// Widget for status message modal overlay (used for warnings)
pub struct StatusModalWidget<'a> {
    message: &'a str,
}

impl<'a> StatusModalWidget<'a> {
    pub fn new(message: &'a str) -> Self {
        Self { message }
    }
}

impl Widget for StatusModalWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Calculate centered box size
        let msg_width = self.message.len() as u16 + 4;
        let box_width = msg_width.max(20).min(area.width.saturating_sub(4));
        let box_height = 3;

        let x = area.x + (area.width.saturating_sub(box_width)) / 2;
        let y = area.y + (area.height.saturating_sub(box_height)) / 2;

        let modal_area = Rect::new(x, y, box_width, box_height);

        // Clear the area behind the modal
        for row in modal_area.y..modal_area.y + modal_area.height {
            for col in modal_area.x..modal_area.x + modal_area.width {
                buf[(col, row)].set_char(' ');
                buf[(col, row)].set_style(Style::default());
            }
        }

        // Use yellow border for warnings
        let border_color = if self.message.starts_with(WARNING_CHAR) {
            Color::Yellow
        } else {
            Color::White
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default());

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        // Center the message
        let msg_x = inner.x + (inner.width.saturating_sub(self.message.len() as u16)) / 2;

        // Use yellow text for warnings
        let text_color = if self.message.starts_with(WARNING_CHAR) {
            Color::Yellow
        } else {
            Color::White
        };

        buf.set_line(
            msg_x,
            inner.y,
            &Line::from(Span::styled(
                self.message,
                Style::default().fg(text_color).add_modifier(Modifier::BOLD),
            )),
            inner.width,
        );
    }
}

/// Widget for rendering the group list
pub struct GroupListWidget<'a> {
    app: &'a App,
    scroll_offset: usize,
}

impl<'a> GroupListWidget<'a> {
    pub fn new(app: &'a App, scroll_offset: usize) -> Self {
        Self { app, scroll_offset }
    }
}

impl Widget for GroupListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mode_str = match self.app.group_mode {
            GroupMode::BySenderEmail => "email",
            GroupMode::ByDomain => "domain",
        };
        let filter_indicator = if self.app.filter_to_threads {
            " [Threads]"
        } else {
            ""
        };
        let filtered_groups = self.app.filtered_groups();
        let total_emails: usize = filtered_groups.iter().map(|g| g.count()).sum();
        let title = format!(
            " Senders (by {}){} — {} emails in {} groups ",
            mode_str,
            filter_indicator,
            total_emails,
            filtered_groups.len()
        );
        let block = Block::default().borders(Borders::ALL).title(title);

        let inner = block.inner(area);
        block.render(area, buf);

        // Get the currently selected group to match by key
        let selected_key = self.app.groups.get(self.app.selected_group).map(|g| &g.key);

        for (i, group) in filtered_groups.iter().enumerate().skip(self.scroll_offset) {
            let row_index = i - self.scroll_offset;
            if row_index >= inner.height as usize {
                break;
            }

            let is_selected = selected_key.is_some_and(|k| k == &group.key);
            let style = if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let thread_indicator = if self.app.group_has_multi_message_threads(group) {
                "◈ "
            } else {
                "  "
            };

            let thread_count = group.thread_count();
            let email_count = group.count();
            let line = if thread_count == email_count {
                // Each email is its own thread
                format!("{}{} ({} emails)", thread_indicator, group.key, email_count)
            } else {
                format!(
                    "{}{} ({} emails in {} threads)",
                    thread_indicator, group.key, email_count, thread_count
                )
            };
            let span = Span::styled(line, style);

            buf.set_line(
                inner.x,
                inner.y + row_index as u16,
                &Line::from(span),
                inner.width,
            );
        }
    }
}

/// Widget for rendering the email list within a group
pub struct EmailListWidget<'a> {
    app: &'a App,
}

impl<'a> EmailListWidget<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl StatefulWidget for EmailListWidget<'_> {
    type State = TableState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let filter_indicator = if self.app.filter_to_threads {
            " [Threads]"
        } else {
            ""
        };
        let title = self
            .app
            .current_group()
            .map(|g| {
                let email_count = self.app.total_thread_emails_for_group(g);
                let thread_count = g.thread_count();
                if thread_count == email_count {
                    format!(
                        " Threads from {}{} — {} threads ",
                        g.key, filter_indicator, thread_count
                    )
                } else {
                    format!(
                        " Threads from {}{} — {} threads ({} emails) ",
                        g.key, filter_indicator, thread_count, email_count
                    )
                }
            })
            .unwrap_or_else(|| " Threads ".to_string());

        let block = Block::default().borders(Borders::ALL).title(title);

        let inner = block.inner(area);
        block.render(area, buf);

        // Use filtered threads (respects filter_to_threads setting)
        let filtered_threads = self.app.filtered_threads_in_current_group();

        // Show message if filter is active but no threads match
        if filtered_threads.is_empty() && self.app.filter_to_threads {
            let msg = "No threads in this group (press t to show all)";
            let x = inner.x + (inner.width.saturating_sub(msg.len() as u16)) / 2;
            let y = inner.y + inner.height / 2;
            buf.set_line(
                x,
                y,
                &Line::from(Span::styled(msg, Style::default().fg(Color::DarkGray))),
                inner.width,
            );
            return;
        }

        // Display one row per thread (newest email in each thread)
        let rows: Vec<Row> = filtered_threads
            .iter()
            .map(|email| {
                let has_multiple_messages = self.app.thread_has_multiple_messages(&email.thread_id);

                let thread_indicator = if has_multiple_messages { "◈" } else { " " };
                let date_str = format_date(&email.date);

                Row::new(vec![
                    date_str,
                    thread_indicator.to_string(),
                    email.subject.clone(),
                ])
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(12), // Date column
                Constraint::Length(1),  // Thread indicator
                Constraint::Min(20),    // Subject
            ],
        )
        .row_highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

        StatefulWidget::render(table, inner, buf, state);
    }
}

/// Widget for rendering the thread view (all emails in a thread)
pub struct ThreadViewWidget<'a> {
    app: &'a App,
}

impl<'a> ThreadViewWidget<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl StatefulWidget for ThreadViewWidget<'_> {
    type State = TableState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let thread_emails = self.app.current_thread_emails();
        let thread_count = thread_emails.len();
        let title = self
            .app
            .current_email()
            .map(|e| {
                if thread_count == 1 {
                    format!(" Thread: {} — 1 email ", e.subject)
                } else {
                    format!(" Thread: {} — {} emails ", e.subject, thread_count)
                }
            })
            .unwrap_or_else(|| " Thread ".to_string());

        let block = Block::default().borders(Borders::ALL).title(title);

        let inner = block.inner(area);
        block.render(area, buf);

        // thread_emails already sorted by date descending from above
        let current_sender = self.app.current_email().map(|e| &e.from_email);

        let rows: Vec<Row> = thread_emails
            .iter()
            .map(|email| {
                let is_other_sender = current_sender.is_some_and(|s| s != &email.from_email);

                let style = if is_other_sender {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };

                let date_str = format_date(&email.date);

                Row::new(vec![
                    date_str,
                    email.from_email.clone(),
                    email.subject.clone(),
                ])
                .style(style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(12), // Date column
                Constraint::Length(30), // Sender email
                Constraint::Min(20),    // Subject
            ],
        )
        .row_highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

        StatefulWidget::render(table, inner, buf, state);
    }
}

/// Widget for rendering the undo history list
pub struct UndoHistoryWidget<'a> {
    app: &'a App,
    scroll_offset: usize,
}

impl<'a> UndoHistoryWidget<'a> {
    pub fn new(app: &'a App, scroll_offset: usize) -> Self {
        Self { app, scroll_offset }
    }

    /// Formats an undo entry for display
    fn format_entry(entry: &crate::app::UndoEntry) -> String {
        let action_icon = match entry.action_type {
            UndoActionType::Archive => "📦",
            UndoActionType::Delete => "🗑️",
        };

        let action_verb = match entry.action_type {
            UndoActionType::Archive => "archived",
            UndoActionType::Delete => "deleted",
        };

        let email_count = entry.emails.len();
        let email_word = if email_count == 1 { "email" } else { "emails" };

        match &entry.context {
            UndoContext::SingleEmail { subject } => {
                let truncated = if subject.len() > 40 {
                    format!("{}...", &subject[..37])
                } else {
                    subject.clone()
                };
                format!("{} {} '{}' (1 email)", action_icon, action_verb, truncated)
            }
            UndoContext::Group { sender } => {
                format!(
                    "{} {} {} {} from {}",
                    action_icon, action_verb, email_count, email_word, sender
                )
            }
            UndoContext::Thread { subject } => {
                let truncated = if subject.len() > 30 {
                    format!("{}...", &subject[..27])
                } else {
                    subject.clone()
                };
                format!(
                    "{} {} thread '{}' ({} {})",
                    action_icon, action_verb, truncated, email_count, email_word
                )
            }
        }
    }
}

impl Widget for UndoHistoryWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Calculate centered modal area (80% width, 60% height)
        let modal_width = (area.width as f32 * 0.8) as u16;
        let modal_height = (area.height as f32 * 0.6) as u16;
        let modal_x = area.x + (area.width.saturating_sub(modal_width)) / 2;
        let modal_y = area.y + (area.height.saturating_sub(modal_height)) / 2;
        let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

        // Clear the background behind the modal
        for row in modal_area.y..modal_area.y + modal_area.height {
            for col in modal_area.x..modal_area.x + modal_area.width {
                buf[(col, row)].set_char(' ');
                buf[(col, row)].set_style(Style::default());
            }
        }

        let title = format!(" Undo History — {} actions ", self.app.undo_history.len());
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default());

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        if self.app.undo_history.is_empty() {
            let msg = "No actions to undo";
            let x = inner.x + (inner.width.saturating_sub(msg.len() as u16)) / 2;
            let y = inner.y + inner.height / 2;
            buf.set_line(
                x,
                y,
                &Line::from(Span::styled(msg, Style::default().fg(Color::DarkGray))),
                inner.width,
            );
            return;
        }

        for (i, entry) in self
            .app
            .undo_history
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
        {
            let row_index = i - self.scroll_offset;
            if row_index >= inner.height as usize {
                break;
            }

            let is_selected = i == self.app.selected_undo;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let line = Self::format_entry(entry);
            let span = Span::styled(line, style);

            buf.set_line(
                inner.x,
                inner.y + row_index as u16,
                &Line::from(span),
                inner.width,
            );
        }
    }
}

/// Widget for the help bar at the bottom
pub struct HelpBarWidget<'a> {
    app: &'a App,
}

impl<'a> HelpBarWidget<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for HelpBarWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let undo_hint = if self.app.undo_history_len() > 0 {
            " | u: undo"
        } else {
            ""
        };

        let help_text = match self.app.view {
            View::GroupList => {
                format!(
                    "j/↓: next | k/↑: prev | Enter: open | t: threads only | m: toggle mode | r: refresh{} | q: quit",
                    undo_hint
                )
            }
            View::EmailList => {
                format!(
                    "j/↓: next | k/↑: prev | Enter: thread | a/A: archive | d/D: delete | t: threads only | m: toggle{} | q: back",
                    undo_hint
                )
            }
            View::Thread => {
                format!(
                    "j/↓: next | k/↑: prev | Enter: browser | A: archive | D: delete{} | q: back",
                    undo_hint
                )
            }
            View::UndoHistory => {
                "j/↓: next | k/↑: prev | Enter: undo selected | q: back".to_string()
            }
        };

        let paragraph = Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray));

        paragraph.render(area, buf);
    }
}

/// Widget for the confirmation dialog
pub struct ConfirmDialogWidget<'a> {
    action: &'a ConfirmAction,
}

impl<'a> ConfirmDialogWidget<'a> {
    pub fn new(action: &'a ConfirmAction) -> Self {
        Self { action }
    }
}

impl Widget for ConfirmDialogWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        use unicode_width::UnicodeWidthStr;

        let message = self.action.message();
        let msg_width = message.width() as u16;

        // Calculate box size based on content (message + horizontal and vertical padding)
        let horizontal_padding = 4_u16; // 2 chars on each side
        let vertical_padding = 2_u16; // 1 line above and below
        let box_width = (msg_width + horizontal_padding + 2)
            .max(20)
            .min(area.width.saturating_sub(4));
        let box_height = 3 + vertical_padding; // border + padding + content + padding + border

        // Center the box
        let x = area.x + (area.width.saturating_sub(box_width)) / 2;
        let y = area.y + (area.height.saturating_sub(box_height)) / 2;

        let modal_area = Rect::new(x, y, box_width, box_height);

        // Clear the area behind the modal
        for row in modal_area.y..modal_area.y + modal_area.height {
            for col in modal_area.x..modal_area.x + modal_area.width {
                buf[(col, row)].set_char(' ');
                buf[(col, row)].set_style(Style::default());
            }
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Confirm ")
            .border_style(Style::default().fg(Color::Red));

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        // Center the message horizontally and vertically within the inner area
        let msg_x = inner.x + inner.width.saturating_sub(msg_width) / 2;
        let msg_y = inner.y + inner.height / 2;

        buf.set_line(
            msg_x,
            msg_y,
            &Line::from(Span::styled(message, Style::default().fg(Color::White))),
            inner.width,
        );
    }
}

/// State for account selection
#[derive(Debug)]
pub struct AccountSelection {
    /// List of (account_name, account_config) pairs
    pub accounts: Vec<(String, AccountConfig)>,
    /// Currently selected index
    pub selected: usize,
}

impl AccountSelection {
    pub fn new(accounts: Vec<(String, AccountConfig)>) -> Self {
        Self {
            accounts,
            selected: 0,
        }
    }

    pub fn select_next(&mut self) {
        if self.selected < self.accounts.len().saturating_sub(1) {
            self.selected += 1;
        }
    }

    pub fn select_previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn current_account(&self) -> Option<&(String, AccountConfig)> {
        self.accounts.get(self.selected)
    }
}

/// Widget for account selection
pub struct AccountSelectWidget<'a> {
    selection: &'a AccountSelection,
}

impl<'a> AccountSelectWidget<'a> {
    pub fn new(selection: &'a AccountSelection) -> Self {
        Self { selection }
    }
}

impl Widget for AccountSelectWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Select Account ");

        let inner = block.inner(area);
        block.render(area, buf);

        for (i, (name, account)) in self.selection.accounts.iter().enumerate() {
            if i >= inner.height.saturating_sub(2) as usize {
                break;
            }

            let style = if i == self.selection.selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let line = format!("{} ({})", name, account.email);
            let span = Span::styled(line, style);

            buf.set_line(inner.x, inner.y + i as u16, &Line::from(span), inner.width);
        }

        // Help text at the bottom
        let help_y = inner.y + inner.height.saturating_sub(1);
        let help_text = "j/↓: next | k/↑: prev | Enter: select | q: quit";
        buf.set_line(
            inner.x,
            help_y,
            &Line::from(Span::styled(
                help_text,
                Style::default().fg(Color::DarkGray),
            )),
            inner.width,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confirm_action_archive_emails() {
        let action = ConfirmAction::ArchiveEmails {
            sender: "test@example.com".to_string(),
            count: 5,
        };
        let msg = action.message();
        assert!(msg.contains("Archive 5 email(s)"));
        assert!(msg.contains("test@example.com"));
    }

    #[test]
    fn test_confirm_action_delete_emails() {
        let action = ConfirmAction::DeleteEmails {
            sender: "test@example.com".to_string(),
            count: 3,
        };
        let msg = action.message();
        assert!(msg.contains("Delete 3 email(s)"));
        assert!(msg.contains("test@example.com"));
    }

    #[test]
    fn test_confirm_action_delete_thread() {
        let action = ConfirmAction::DeleteThread {
            thread_email_count: 3,
        };
        let msg = action.message();
        assert!(msg.contains("Delete entire thread"));
        assert!(msg.contains("3 email(s)"));
    }

    #[test]
    fn test_confirm_action_quit() {
        let action = ConfirmAction::Quit;
        let msg = action.message();
        assert!(msg.contains("Quit"));
    }

    #[test]
    fn test_ui_state_confirm_flow() {
        let mut state = UiState::new();
        assert!(!state.is_confirming());

        state.set_confirm(ConfirmAction::ArchiveThread {
            thread_email_count: 1,
        });
        assert!(state.is_confirming());

        state.clear_confirm();
        assert!(!state.is_confirming());
    }

    #[test]
    fn test_ui_state_status() {
        let mut state = UiState::new();
        assert!(state.status_message.is_none());

        state.set_status("Loading...");
        assert_eq!(state.status_message, Some("Loading...".to_string()));

        state.clear_status();
        assert!(state.status_message.is_none());
    }
}
