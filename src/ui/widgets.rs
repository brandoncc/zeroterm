use chrono::{DateTime, Datelike, Local, Utc};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, Widget},
};

use crate::app::{App, GroupMode, ThreadImpact, ThreadWarning, View};
use crate::config::AccountConfig;

/// Warning indicator character for messages
const WARNING_CHAR: char = '⚠';

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
    ArchiveEmails {
        sender: String,
        count: usize,
        impact: ThreadImpact,
    },
    /// Delete emails from a sender (only their emails, not full threads)
    DeleteEmails {
        sender: String,
        count: usize,
        impact: ThreadImpact,
    },
    /// Archive entire thread (all emails including other senders)
    ArchiveThread { thread_email_count: usize },
    /// Delete entire thread (all emails including other senders)
    DeleteThread { thread_email_count: usize },
}

impl ConfirmAction {
    pub fn message(&self) -> Vec<String> {
        match self {
            ConfirmAction::ArchiveEmails {
                sender,
                count,
                impact,
            } => {
                let mut lines = vec![format!("Archive {} email(s) from {}?", count, sender)];
                if let Some(warning) = &impact.warning {
                    match warning {
                        ThreadWarning::SenderEmailMode {
                            thread_count,
                            email_count,
                        } => {
                            lines.push(format!(
                                "{} {} thread(s) also contain {} email(s) from other senders",
                                WARNING_CHAR, thread_count, email_count
                            ));
                            lines.push(
                                "(only emails from this sender will be archived)".to_string(),
                            );
                        }
                        ThreadWarning::DomainMode { thread_count } => {
                            lines.push(format!(
                                "{} {} thread(s) have multiple participants",
                                WARNING_CHAR, thread_count
                            ));
                        }
                    }
                }
                lines.push("(y/n)".to_string());
                lines
            }
            ConfirmAction::DeleteEmails {
                sender,
                count,
                impact,
            } => {
                let mut lines = vec![format!("Delete {} email(s) from {}?", count, sender)];
                if let Some(warning) = &impact.warning {
                    match warning {
                        ThreadWarning::SenderEmailMode {
                            thread_count,
                            email_count,
                        } => {
                            lines.push(format!(
                                "{} {} thread(s) also contain {} email(s) from other senders",
                                WARNING_CHAR, thread_count, email_count
                            ));
                            lines
                                .push("(only emails from this sender will be deleted)".to_string());
                        }
                        ThreadWarning::DomainMode { thread_count } => {
                            lines.push(format!(
                                "{} {} thread(s) have multiple participants",
                                WARNING_CHAR, thread_count
                            ));
                        }
                    }
                }
                lines.push("(y/n)".to_string());
                lines
            }
            ConfirmAction::ArchiveThread { thread_email_count } => {
                vec![
                    format!("Archive entire thread ({} email(s))?", thread_email_count),
                    "(y/n)".to_string(),
                ]
            }
            ConfirmAction::DeleteThread { thread_email_count } => {
                vec![
                    format!("Delete entire thread ({} email(s))?", thread_email_count),
                    "(y/n)".to_string(),
                ]
            }
        }
    }
}

/// Spinner frames for animated busy indicator
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// UI state that supplements App state
#[derive(Debug, Default)]
pub struct UiState {
    pub confirm_action: Option<ConfirmAction>,
    pub status_message: Option<String>,
    /// When true, the UI is busy with an IMAP operation and input is blocked
    pub busy: bool,
    /// Frame counter for spinner animation
    pub spinner_frame: usize,
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

/// Widget for rendering the group list
pub struct GroupListWidget<'a> {
    app: &'a App,
}

impl<'a> GroupListWidget<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for GroupListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mode_str = match self.app.group_mode {
            GroupMode::BySenderEmail => "email",
            GroupMode::ByDomain => "domain",
        };
        let title = format!(" Senders (by {}) ", mode_str);
        let block = Block::default().borders(Borders::ALL).title(title);

        let inner = block.inner(area);
        block.render(area, buf);

        for (i, group) in self.app.groups.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }

            let style = if i == self.app.selected_group {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let thread_count = group.thread_count();
            let email_count = group.count();
            let line = if thread_count == email_count {
                // Each email is its own thread
                format!("{} ({} emails)", group.key, email_count)
            } else {
                format!(
                    "{} ({} emails in {} threads)",
                    group.key, email_count, thread_count
                )
            };
            let span = Span::styled(line, style);

            buf.set_line(inner.x, inner.y + i as u16, &Line::from(span), inner.width);
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

impl Widget for EmailListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = self
            .app
            .current_group()
            .map(|g| format!(" Emails from {} ", g.key))
            .unwrap_or_else(|| " Emails ".to_string());

        let block = Block::default().borders(Borders::ALL).title(title);

        let inner = block.inner(area);
        block.render(area, buf);

        if let Some(group) = self.app.current_group() {
            // Emails are already sorted by date descending in the group
            let rows: Vec<Row> = group
                .emails
                .iter()
                .enumerate()
                .map(|(i, email)| {
                    let is_selected = self.app.selected_email == Some(i);
                    let has_other_senders = self.app.thread_has_multiple_senders(&email.thread_id);

                    let style = if is_selected {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };

                    let thread_indicator = if has_other_senders { "◈" } else { " " };
                    let date_str = format_date(&email.date);

                    Row::new(vec![
                        date_str,
                        thread_indicator.to_string(),
                        email.subject.clone(),
                    ])
                    .style(style)
                })
                .collect();

            let table = Table::new(
                rows,
                [
                    Constraint::Length(12), // Date column
                    Constraint::Length(1),  // Thread indicator
                    Constraint::Min(20),    // Subject
                ],
            );

            Widget::render(table, inner, buf);
        }
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

impl Widget for ThreadViewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = self
            .app
            .current_email()
            .map(|e| format!(" Thread: {} ", e.subject))
            .unwrap_or_else(|| " Thread ".to_string());

        let block = Block::default().borders(Borders::ALL).title(title);

        let inner = block.inner(area);
        block.render(area, buf);

        // Thread emails are already sorted by date descending
        let thread_emails = self.app.current_thread_emails();
        let current_sender = self.app.current_email().map(|e| &e.from_email);

        let rows: Vec<Row> = thread_emails
            .iter()
            .enumerate()
            .map(|(i, email)| {
                let is_selected = self.app.selected_thread_email == Some(i);
                let is_other_sender = current_sender.is_some_and(|s| s != &email.from_email);

                let style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if is_other_sender {
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
        );

        Widget::render(table, inner, buf);
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
        let help_text = match self.app.view {
            View::GroupList => {
                "j/↓: next | k/↑: prev | Enter: open | A: archive all | D: delete all | g: toggle mode | r: refresh | q: quit"
            }
            View::EmailList => {
                "j/↓: next | k/↑: prev | Enter: view thread | a: archive | A: archive all | d: delete | D: delete all | g: toggle | q: back"
            }
            View::Thread => {
                "j/↓: next | k/↑: prev | Enter: open in browser | A: archive thread | D: delete thread | q: back"
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
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Confirm ")
            .style(Style::default().fg(Color::Red));

        let inner = block.inner(area);
        block.render(area, buf);

        let lines = self.action.message();
        for (i, line) in lines.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }

            let style = if line.starts_with(WARNING_CHAR) {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            buf.set_line(
                inner.x,
                inner.y + i as u16,
                &Line::from(Span::styled(line.clone(), style)),
                inner.width,
            );
        }
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
impl UiState {
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ThreadImpact, ThreadWarning};

    #[test]
    fn test_confirm_action_archive_no_impact() {
        let action = ConfirmAction::ArchiveEmails {
            sender: "test@example.com".to_string(),
            count: 5,
            impact: ThreadImpact { warning: None },
        };
        let lines = action.message();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Archive 5 email(s)"));
        assert!(!lines.iter().any(|l| l.contains(WARNING_CHAR)));
    }

    #[test]
    fn test_confirm_action_archive_with_impact() {
        let action = ConfirmAction::ArchiveEmails {
            sender: "test@example.com".to_string(),
            count: 5,
            impact: ThreadImpact {
                warning: Some(ThreadWarning::SenderEmailMode {
                    thread_count: 2,
                    email_count: 4,
                }),
            },
        };
        let lines = action.message();
        assert!(lines.len() > 2);
        assert!(lines.iter().any(|l| l.contains(WARNING_CHAR)));
        assert!(lines.iter().any(|l| l.contains("2 thread(s)")));
        assert!(lines.iter().any(|l| l.contains("4 email(s) from other")));
    }

    #[test]
    fn test_confirm_action_delete_thread() {
        let action = ConfirmAction::DeleteThread {
            thread_email_count: 3,
        };
        let lines = action.message();
        assert!(lines[0].contains("Delete entire thread"));
        assert!(lines[0].contains("3 email(s)"));
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
