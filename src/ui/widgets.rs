use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::app::{App, View};

/// State for the confirmation dialog
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    ArchiveAll { sender: String, count: usize },
    DeleteAll { sender: String, count: usize },
}

impl ConfirmAction {
    pub fn message(&self) -> String {
        match self {
            ConfirmAction::ArchiveAll { sender, count } => {
                format!("Archive {} emails from {}? (y/n)", count, sender)
            }
            ConfirmAction::DeleteAll { sender, count } => {
                format!("Delete {} emails from {}? (y/n)", count, sender)
            }
        }
    }
}

/// UI state that supplements App state
#[derive(Debug, Default)]
pub struct UiState {
    pub confirm_action: Option<ConfirmAction>,
    pub status_message: Option<String>,
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

    pub fn clear_status(&mut self) {
        self.status_message = None;
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
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Senders (by email) ");

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

            let line = format!("{} ({})", group.key, group.count());
            let span = Span::styled(line, style);

            buf.set_line(
                inner.x,
                inner.y + i as u16,
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
            for (i, email) in group.emails.iter().enumerate() {
                if i >= inner.height as usize {
                    break;
                }

                let is_selected = self.app.selected_email == Some(i);
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                let line = format!("{}: {}", email.from_email, email.subject);
                let span = Span::styled(line, style);

                buf.set_line(
                    inner.x,
                    inner.y + i as u16,
                    &Line::from(span),
                    inner.width,
                );
            }
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
        let help_text = match self.app.view {
            View::GroupList => {
                "j/↓: next | k/↑: prev | Enter: open | A: archive all | D: delete all | g: toggle mode | r: refresh | q: quit"
            }
            View::EmailList => {
                "j/↓: next | k/↑: prev | a: archive | A: archive all | d: delete | D: delete all | g: toggle mode | r: refresh | q: back"
            }
        };

        let paragraph = Paragraph::new(help_text)
            .style(Style::default().fg(Color::DarkGray));

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

        let message = self.action.message();
        let paragraph = Paragraph::new(message)
            .style(Style::default().fg(Color::White));

        paragraph.render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confirm_action_archive_message() {
        let action = ConfirmAction::ArchiveAll {
            sender: "test@example.com".to_string(),
            count: 5,
        };
        assert_eq!(
            action.message(),
            "Archive 5 emails from test@example.com? (y/n)"
        );
    }

    #[test]
    fn test_confirm_action_delete_message() {
        let action = ConfirmAction::DeleteAll {
            sender: "test@example.com".to_string(),
            count: 3,
        };
        assert_eq!(
            action.message(),
            "Delete 3 emails from test@example.com? (y/n)"
        );
    }

    #[test]
    fn test_ui_state_confirm_flow() {
        let mut state = UiState::new();
        assert!(!state.is_confirming());

        state.set_confirm(ConfirmAction::ArchiveAll {
            sender: "test@example.com".to_string(),
            count: 1,
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
