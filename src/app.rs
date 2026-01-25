use crate::email::Email;
use std::collections::HashMap;

/// The grouping mode for emails
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum GroupMode {
    #[default]
    ByEmail,
    ByDomain,
}

/// The current view state
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum View {
    #[default]
    GroupList,
    EmailList,
}

/// Represents a group of emails from the same sender
#[derive(Debug, Clone, PartialEq)]
pub struct EmailGroup {
    pub key: String,
    pub emails: Vec<Email>,
}

impl EmailGroup {
    pub fn new(key: String) -> Self {
        Self {
            key,
            emails: Vec::new(),
        }
    }

    pub fn count(&self) -> usize {
        self.emails.len()
    }
}

/// The main application state
#[derive(Debug)]
pub struct App {
    pub groups: Vec<EmailGroup>,
    pub group_mode: GroupMode,
    pub selected_group: usize,
    pub selected_email: Option<usize>,
    pub view: View,
    emails: Vec<Email>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            groups: Vec::new(),
            group_mode: GroupMode::default(),
            selected_group: 0,
            selected_email: None,
            view: View::default(),
            emails: Vec::new(),
        }
    }

    /// Sets the emails and regroups them according to current mode
    pub fn set_emails(&mut self, emails: Vec<Email>) {
        self.emails = emails;
        self.regroup();
    }

    /// Regroups emails according to the current group mode
    fn regroup(&mut self) {
        let mut group_map: HashMap<String, Vec<Email>> = HashMap::new();

        for email in &self.emails {
            let key = match self.group_mode {
                GroupMode::ByEmail => email.from_email.clone(),
                GroupMode::ByDomain => email.from_domain.clone(),
            };
            group_map.entry(key).or_default().push(email.clone());
        }

        self.groups = group_map
            .into_iter()
            .map(|(key, emails)| {
                let mut group = EmailGroup::new(key);
                group.emails = emails;
                group
            })
            .collect();

        // Sort groups by email count (descending) for better UX
        self.groups.sort_by(|a, b| b.count().cmp(&a.count()));

        // Reset selection if out of bounds
        if self.selected_group >= self.groups.len() && !self.groups.is_empty() {
            self.selected_group = self.groups.len() - 1;
        }
    }

    /// Toggles between ByEmail and ByDomain grouping modes
    pub fn toggle_group_mode(&mut self) {
        self.group_mode = match self.group_mode {
            GroupMode::ByEmail => GroupMode::ByDomain,
            GroupMode::ByDomain => GroupMode::ByEmail,
        };
        self.regroup();
        self.selected_group = 0;
        self.selected_email = None;
    }

    /// Selects the next group in the list
    pub fn select_next_group(&mut self) {
        if !self.groups.is_empty() && self.selected_group < self.groups.len() - 1 {
            self.selected_group += 1;
        }
    }

    /// Selects the previous group in the list
    pub fn select_previous_group(&mut self) {
        if self.selected_group > 0 {
            self.selected_group -= 1;
        }
    }

    /// Selects the next email in the current group
    pub fn select_next_email(&mut self) {
        if let Some(group) = self.groups.get(self.selected_group) {
            if group.emails.is_empty() {
                return;
            }

            self.selected_email = match self.selected_email {
                Some(idx) if idx < group.emails.len() - 1 => Some(idx + 1),
                Some(idx) => Some(idx),
                None => Some(0),
            };
        }
    }

    /// Selects the previous email in the current group
    pub fn select_previous_email(&mut self) {
        if self.groups.get(self.selected_group).is_some() {
            self.selected_email = match self.selected_email {
                Some(idx) if idx > 0 => Some(idx - 1),
                Some(idx) => Some(idx),
                None => None,
            };
        }
    }

    /// Enters the email list view for the currently selected group
    pub fn enter_group(&mut self) {
        if !self.groups.is_empty() {
            self.view = View::EmailList;
            self.selected_email = if self.groups[self.selected_group].emails.is_empty() {
                None
            } else {
                Some(0)
            };
        }
    }

    /// Returns to the group list view
    pub fn exit_group(&mut self) {
        self.view = View::GroupList;
        self.selected_email = None;
    }

    /// Gets the currently selected group, if any
    pub fn current_group(&self) -> Option<&EmailGroup> {
        self.groups.get(self.selected_group)
    }

    /// Gets the currently selected email, if any
    pub fn current_email(&self) -> Option<&Email> {
        self.current_group()
            .and_then(|g| self.selected_email.and_then(|idx| g.emails.get(idx)))
    }

    /// Removes an email by ID and regroups
    pub fn remove_email(&mut self, email_id: &str) {
        self.emails.retain(|e| e.id != email_id);
        self.regroup();

        // Adjust selected_email if needed
        if let Some(group) = self.groups.get(self.selected_group) {
            if let Some(idx) = self.selected_email {
                if idx >= group.emails.len() {
                    self.selected_email = if group.emails.is_empty() {
                        None
                    } else {
                        Some(group.emails.len() - 1)
                    };
                }
            }
        }
    }

    /// Removes all emails in the current group
    pub fn remove_current_group_emails(&mut self) {
        if let Some(group) = self.groups.get(self.selected_group) {
            let ids_to_remove: Vec<String> = group.emails.iter().map(|e| e.id.clone()).collect();
            self.emails.retain(|e| !ids_to_remove.contains(&e.id));
        }
        self.regroup();
        self.selected_email = None;

        // If we removed the last group, adjust selection
        if self.selected_group >= self.groups.len() && !self.groups.is_empty() {
            self.selected_group = self.groups.len() - 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_email(id: &str, from: &str) -> Email {
        Email::new(
            id.to_string(),
            format!("thread_{id}"),
            from.to_string(),
            "Subject".to_string(),
            "Snippet".to_string(),
            Utc::now(),
        )
    }

    #[test]
    fn test_app_default_state() {
        let app = App::new();
        assert_eq!(app.group_mode, GroupMode::ByEmail);
        assert_eq!(app.view, View::GroupList);
        assert_eq!(app.selected_group, 0);
        assert_eq!(app.selected_email, None);
        assert!(app.groups.is_empty());
    }

    #[test]
    fn test_group_by_email() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "bob@example.com"),
            create_test_email("3", "alice@example.com"),
        ]);

        assert_eq!(app.groups.len(), 2);

        // Find alice's group (should have 2 emails)
        let alice_group = app.groups.iter().find(|g| g.key == "alice@example.com");
        assert!(alice_group.is_some());
        assert_eq!(alice_group.unwrap().count(), 2);

        // Find bob's group (should have 1 email)
        let bob_group = app.groups.iter().find(|g| g.key == "bob@example.com");
        assert!(bob_group.is_some());
        assert_eq!(bob_group.unwrap().count(), 1);
    }

    #[test]
    fn test_group_by_domain() {
        let mut app = App::new();
        app.group_mode = GroupMode::ByDomain;
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "bob@other.com"),
            create_test_email("3", "charlie@example.com"),
        ]);

        assert_eq!(app.groups.len(), 2);

        // example.com should have 2 emails
        let example_group = app.groups.iter().find(|g| g.key == "example.com");
        assert!(example_group.is_some());
        assert_eq!(example_group.unwrap().count(), 2);
    }

    #[test]
    fn test_toggle_group_mode() {
        let mut app = App::new();
        assert_eq!(app.group_mode, GroupMode::ByEmail);

        app.toggle_group_mode();
        assert_eq!(app.group_mode, GroupMode::ByDomain);

        app.toggle_group_mode();
        assert_eq!(app.group_mode, GroupMode::ByEmail);
    }

    #[test]
    fn test_navigation_groups() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "a@test.com"),
            create_test_email("2", "b@test.com"),
            create_test_email("3", "c@test.com"),
        ]);

        assert_eq!(app.selected_group, 0);

        app.select_next_group();
        assert_eq!(app.selected_group, 1);

        app.select_next_group();
        assert_eq!(app.selected_group, 2);

        // Should not go beyond bounds
        app.select_next_group();
        assert_eq!(app.selected_group, 2);

        app.select_previous_group();
        assert_eq!(app.selected_group, 1);

        app.select_previous_group();
        assert_eq!(app.selected_group, 0);

        // Should not go below 0
        app.select_previous_group();
        assert_eq!(app.selected_group, 0);
    }

    #[test]
    fn test_navigation_emails() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
            create_test_email("3", "alice@example.com"),
        ]);

        // Enter the group
        app.enter_group();
        assert_eq!(app.view, View::EmailList);
        assert_eq!(app.selected_email, Some(0));

        app.select_next_email();
        assert_eq!(app.selected_email, Some(1));

        app.select_next_email();
        assert_eq!(app.selected_email, Some(2));

        // Should not go beyond bounds
        app.select_next_email();
        assert_eq!(app.selected_email, Some(2));

        app.select_previous_email();
        assert_eq!(app.selected_email, Some(1));

        app.select_previous_email();
        assert_eq!(app.selected_email, Some(0));

        // Should not go below 0
        app.select_previous_email();
        assert_eq!(app.selected_email, Some(0));
    }

    #[test]
    fn test_enter_and_exit_group() {
        let mut app = App::new();
        app.set_emails(vec![create_test_email("1", "alice@example.com")]);

        assert_eq!(app.view, View::GroupList);

        app.enter_group();
        assert_eq!(app.view, View::EmailList);
        assert_eq!(app.selected_email, Some(0));

        app.exit_group();
        assert_eq!(app.view, View::GroupList);
        assert_eq!(app.selected_email, None);
    }

    #[test]
    fn test_current_group_and_email() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
        ]);

        assert!(app.current_group().is_some());
        assert!(app.current_email().is_none());

        app.enter_group();
        assert!(app.current_email().is_some());
        assert_eq!(app.current_email().unwrap().id, app.groups[0].emails[0].id);
    }

    #[test]
    fn test_remove_email() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
            create_test_email("3", "bob@example.com"),
        ]);

        let initial_alice_count = app
            .groups
            .iter()
            .find(|g| g.key == "alice@example.com")
            .map(|g| g.count())
            .unwrap_or(0);
        assert_eq!(initial_alice_count, 2);

        app.remove_email("1");

        let new_alice_count = app
            .groups
            .iter()
            .find(|g| g.key == "alice@example.com")
            .map(|g| g.count())
            .unwrap_or(0);
        assert_eq!(new_alice_count, 1);
    }

    #[test]
    fn test_remove_current_group_emails() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
            create_test_email("3", "bob@example.com"),
        ]);

        // Select the group with the most emails (alice)
        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;

        app.remove_current_group_emails();

        // Alice's group should be gone
        assert!(app
            .groups
            .iter()
            .find(|g| g.key == "alice@example.com")
            .is_none());

        // Bob's group should remain
        assert!(app
            .groups
            .iter()
            .find(|g| g.key == "bob@example.com")
            .is_some());
    }

    #[test]
    fn test_groups_sorted_by_count() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
            create_test_email("3", "alice@example.com"),
            create_test_email("4", "bob@example.com"),
        ]);

        // Groups should be sorted by count descending
        assert_eq!(app.groups[0].count(), 3); // alice
        assert_eq!(app.groups[1].count(), 1); // bob
    }
}
