use crate::email::Email;
use std::collections::{HashMap, HashSet};

/// The grouping mode for emails
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum GroupMode {
    #[default]
    BySenderEmail,
    ByDomain,
}

/// The current view state
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum View {
    #[default]
    GroupList,
    EmailList,
    Thread,
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

    /// Returns the number of unique threads in this group
    pub fn thread_count(&self) -> usize {
        self.emails
            .iter()
            .map(|e| &e.thread_id)
            .collect::<HashSet<_>>()
            .len()
    }
}

/// Warning about threads with multiple participants
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadWarning {
    /// Sender email grouping mode: threads contain emails from other senders
    SenderEmailMode {
        thread_count: usize,
        email_count: usize,
    },
    /// Domain grouping mode: threads have multiple participants
    DomainMode { thread_count: usize },
}

/// Information about threads that would be affected by an action
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ThreadImpact {
    pub warning: Option<ThreadWarning>,
}

/// The main application state
#[derive(Debug)]
pub struct App {
    pub groups: Vec<EmailGroup>,
    pub group_mode: GroupMode,
    pub selected_group: usize,
    pub selected_email: Option<usize>,
    pub selected_thread_email: Option<usize>,
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
            selected_thread_email: None,
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
                GroupMode::BySenderEmail => email.from_email.clone(),
                GroupMode::ByDomain => email.from_domain.clone(),
            };
            group_map.entry(key).or_default().push(email.clone());
        }

        self.groups = group_map
            .into_iter()
            .map(|(key, mut emails)| {
                // Sort emails by date descending (newest first)
                emails.sort_by(|a, b| b.date.cmp(&a.date));
                let mut group = EmailGroup::new(key);
                group.emails = emails;
                group
            })
            .collect();

        // Sort groups by email count (descending) for better UX
        self.groups.sort_by_key(|g| std::cmp::Reverse(g.count()));

        // Reset selection if out of bounds
        if self.selected_group >= self.groups.len() && !self.groups.is_empty() {
            self.selected_group = self.groups.len() - 1;
        }
    }

    /// Toggles between BySenderEmail and ByDomain grouping modes
    pub fn toggle_group_mode(&mut self) {
        self.group_mode = match self.group_mode {
            GroupMode::BySenderEmail => GroupMode::ByDomain,
            GroupMode::ByDomain => GroupMode::BySenderEmail,
        };
        self.regroup();
        self.selected_group = 0;
        self.selected_email = None;
        self.selected_thread_email = None;
    }

    /// Selects the next item based on current view
    pub fn select_next(&mut self) {
        match self.view {
            View::GroupList => self.select_next_group(),
            View::EmailList => self.select_next_email(),
            View::Thread => self.select_next_thread_email(),
        }
    }

    /// Selects the previous item based on current view
    pub fn select_previous(&mut self) {
        match self.view {
            View::GroupList => self.select_previous_group(),
            View::EmailList => self.select_previous_email(),
            View::Thread => self.select_previous_thread_email(),
        }
    }

    /// Moves selection down by n items (clamped to list bounds)
    pub fn select_next_n(&mut self, n: usize) {
        for _ in 0..n {
            self.select_next();
        }
    }

    /// Moves selection up by n items (clamped to list bounds)
    pub fn select_previous_n(&mut self, n: usize) {
        for _ in 0..n {
            self.select_previous();
        }
    }

    /// Selects the next group in the list
    fn select_next_group(&mut self) {
        if !self.groups.is_empty() && self.selected_group < self.groups.len() - 1 {
            self.selected_group += 1;
        }
    }

    /// Selects the previous group in the list
    fn select_previous_group(&mut self) {
        if self.selected_group > 0 {
            self.selected_group -= 1;
        }
    }

    /// Selects the next email in the current group
    fn select_next_email(&mut self) {
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
    fn select_previous_email(&mut self) {
        if self.groups.get(self.selected_group).is_some() {
            self.selected_email = match self.selected_email {
                Some(idx) if idx > 0 => Some(idx - 1),
                Some(idx) => Some(idx),
                None => None,
            };
        }
    }

    /// Selects the next email in thread view
    fn select_next_thread_email(&mut self) {
        let thread_emails = self.current_thread_emails();
        if thread_emails.is_empty() {
            return;
        }

        self.selected_thread_email = match self.selected_thread_email {
            Some(idx) if idx < thread_emails.len() - 1 => Some(idx + 1),
            Some(idx) => Some(idx),
            None => Some(0),
        };
    }

    /// Selects the previous email in thread view
    fn select_previous_thread_email(&mut self) {
        self.selected_thread_email = match self.selected_thread_email {
            Some(idx) if idx > 0 => Some(idx - 1),
            Some(idx) => Some(idx),
            None => None,
        };
    }

    /// Enters the next view level (group -> emails -> thread)
    pub fn enter(&mut self) {
        match self.view {
            View::GroupList => self.enter_group(),
            View::EmailList => self.enter_thread(),
            View::Thread => {} // Already at deepest level
        }
    }

    /// Exits to the previous view level (thread -> emails -> group)
    pub fn exit(&mut self) {
        match self.view {
            View::GroupList => {} // Can't go back, handled by main.rs for quit
            View::EmailList => self.exit_to_groups(),
            View::Thread => self.exit_to_emails(),
        }
    }

    /// Enters the email list view for the currently selected group
    fn enter_group(&mut self) {
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
    fn exit_to_groups(&mut self) {
        self.view = View::GroupList;
        self.selected_email = None;
    }

    /// Enters the thread view for the currently selected email
    fn enter_thread(&mut self) {
        if self.current_email().is_some() {
            self.view = View::Thread;
            self.selected_thread_email = Some(0);
        }
    }

    /// Returns to the email list view
    fn exit_to_emails(&mut self) {
        self.view = View::EmailList;
        self.selected_thread_email = None;
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

    /// Gets all emails in the thread of the currently selected email
    pub fn current_thread_emails(&self) -> Vec<&Email> {
        let Some(current) = self.current_email() else {
            return Vec::new();
        };

        let thread_id = &current.thread_id;
        let mut thread_emails: Vec<&Email> = self
            .emails
            .iter()
            .filter(|e| &e.thread_id == thread_id)
            .collect();

        // Sort by date descending (newest first)
        thread_emails.sort_by(|a, b| b.date.cmp(&a.date));
        thread_emails
    }

    /// Checks if a thread has emails from multiple senders
    pub fn thread_has_multiple_senders(&self, thread_id: &str) -> bool {
        let senders: HashSet<&str> = self
            .emails
            .iter()
            .filter(|e| e.thread_id == thread_id)
            .map(|e| e.from_email.as_str())
            .collect();
        senders.len() > 1
    }

    /// Checks if any email in a group is part of a multi-sender thread
    pub fn group_has_multi_sender_threads(&self, group: &EmailGroup) -> bool {
        group
            .emails
            .iter()
            .any(|email| self.thread_has_multiple_senders(&email.thread_id))
    }

    /// Gets the thread IDs for emails in the current group
    pub fn current_group_thread_ids(&self) -> HashSet<String> {
        self.current_group()
            .map(|g| g.emails.iter().map(|e| e.thread_id.clone()).collect())
            .unwrap_or_default()
    }

    /// Calculates the impact of archiving/deleting all emails in the current group
    pub fn current_group_thread_impact(&self) -> ThreadImpact {
        let Some(group) = self.current_group() else {
            return ThreadImpact::default();
        };

        let group_key = &group.key;
        let thread_ids = self.current_group_thread_ids();

        let mut multi_sender_threads = 0;
        let mut other_sender_emails = 0;

        for thread_id in &thread_ids {
            let thread_emails: Vec<&Email> = self
                .emails
                .iter()
                .filter(|e| &e.thread_id == thread_id)
                .collect();

            let senders: HashSet<&str> = thread_emails
                .iter()
                .map(|e| e.from_email.as_str())
                .collect();

            if senders.len() > 1 {
                multi_sender_threads += 1;
                if self.group_mode == GroupMode::BySenderEmail {
                    other_sender_emails += thread_emails
                        .iter()
                        .filter(|e| &e.from_email != group_key)
                        .count();
                }
            }
        }

        let warning = if multi_sender_threads > 0 {
            Some(match self.group_mode {
                GroupMode::BySenderEmail => ThreadWarning::SenderEmailMode {
                    thread_count: multi_sender_threads,
                    email_count: other_sender_emails,
                },
                GroupMode::ByDomain => ThreadWarning::DomainMode {
                    thread_count: multi_sender_threads,
                },
            })
        } else {
            None
        };

        ThreadImpact { warning }
    }

    /// Removes an email by ID and regroups
    pub fn remove_email(&mut self, email_id: &str) {
        self.emails.retain(|e| e.id != email_id);
        self.regroup();

        // Adjust selected_email if needed
        if let Some(group) = self.groups.get(self.selected_group)
            && let Some(idx) = self.selected_email
            && idx >= group.emails.len()
        {
            self.selected_email = if group.emails.is_empty() {
                None
            } else {
                Some(group.emails.len() - 1)
            };
        }
    }

    /// Removes all emails in a thread by thread ID
    pub fn remove_thread(&mut self, thread_id: &str) {
        self.emails.retain(|e| e.thread_id != thread_id);
        self.regroup();

        // Adjust selections
        if let Some(group) = self.groups.get(self.selected_group)
            && let Some(idx) = self.selected_email
            && idx >= group.emails.len()
        {
            self.selected_email = if group.emails.is_empty() {
                None
            } else {
                Some(group.emails.len() - 1)
            };
        }
        self.selected_thread_email = None;
    }

    /// Removes all emails in the current group (only emails from this sender)
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

    /// Gets all email IDs in the current group
    pub fn current_group_email_ids(&self) -> Vec<String> {
        self.current_group()
            .map(|g| g.emails.iter().map(|e| e.id.clone()).collect())
            .unwrap_or_default()
    }

    /// Gets all email IDs in the current thread
    pub fn current_thread_email_ids(&self) -> Vec<String> {
        self.current_thread_emails()
            .iter()
            .map(|e| e.id.clone())
            .collect()
    }

    /// Gets the currently selected email in thread view
    pub fn current_thread_email(&self) -> Option<&Email> {
        let thread_emails = self.current_thread_emails();
        self.selected_thread_email
            .and_then(|idx| thread_emails.get(idx).copied())
    }
}

#[cfg(test)]
impl App {
    /// Calculates the impact of archiving/deleting the currently selected email
    pub fn current_email_thread_impact(&self) -> ThreadImpact {
        let Some(email) = self.current_email() else {
            return ThreadImpact::default();
        };

        if self.thread_has_multiple_senders(&email.thread_id) {
            let other_count = self
                .emails
                .iter()
                .filter(|e| e.thread_id == email.thread_id && e.from_email != email.from_email)
                .count();

            ThreadImpact {
                warning: Some(ThreadWarning::SenderEmailMode {
                    thread_count: 1,
                    email_count: other_count,
                }),
            }
        } else {
            ThreadImpact::default()
        }
    }

    /// Removes all emails in threads that contain emails from the current group
    /// This affects ALL emails in those threads, including from other senders
    pub fn remove_current_group_threads(&mut self) {
        let thread_ids = self.current_group_thread_ids();
        self.emails.retain(|e| !thread_ids.contains(&e.thread_id));
        self.regroup();
        self.selected_email = None;
        self.selected_thread_email = None;

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

    fn create_test_email_with_thread(id: &str, thread_id: &str, from: &str) -> Email {
        Email::new(
            id.to_string(),
            thread_id.to_string(),
            from.to_string(),
            "Subject".to_string(),
            "Snippet".to_string(),
            Utc::now(),
        )
    }

    #[test]
    fn test_app_default_state() {
        let app = App::new();
        assert_eq!(app.group_mode, GroupMode::BySenderEmail);
        assert_eq!(app.view, View::GroupList);
        assert_eq!(app.selected_group, 0);
        assert_eq!(app.selected_email, None);
        assert_eq!(app.selected_thread_email, None);
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

        let alice_group = app.groups.iter().find(|g| g.key == "alice@example.com");
        assert!(alice_group.is_some());
        assert_eq!(alice_group.unwrap().count(), 2);

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

        let example_group = app.groups.iter().find(|g| g.key == "example.com");
        assert!(example_group.is_some());
        assert_eq!(example_group.unwrap().count(), 2);
    }

    #[test]
    fn test_toggle_group_mode() {
        let mut app = App::new();
        assert_eq!(app.group_mode, GroupMode::BySenderEmail);

        app.toggle_group_mode();
        assert_eq!(app.group_mode, GroupMode::ByDomain);

        app.toggle_group_mode();
        assert_eq!(app.group_mode, GroupMode::BySenderEmail);
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

        app.select_next();
        assert_eq!(app.selected_group, 1);

        app.select_next();
        assert_eq!(app.selected_group, 2);

        app.select_next();
        assert_eq!(app.selected_group, 2); // Bounds check

        app.select_previous();
        assert_eq!(app.selected_group, 1);

        app.select_previous();
        assert_eq!(app.selected_group, 0);

        app.select_previous();
        assert_eq!(app.selected_group, 0); // Bounds check
    }

    #[test]
    fn test_navigation_emails() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
            create_test_email("3", "alice@example.com"),
        ]);

        app.enter();
        assert_eq!(app.view, View::EmailList);
        assert_eq!(app.selected_email, Some(0));

        app.select_next();
        assert_eq!(app.selected_email, Some(1));

        app.select_next();
        assert_eq!(app.selected_email, Some(2));

        app.select_next();
        assert_eq!(app.selected_email, Some(2)); // Bounds check

        app.select_previous();
        assert_eq!(app.selected_email, Some(1));
    }

    #[test]
    fn test_three_level_navigation() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
        ]);

        // Start at group list
        assert_eq!(app.view, View::GroupList);

        // Enter email list
        app.enter();
        assert_eq!(app.view, View::EmailList);

        // Enter thread view
        app.enter();
        assert_eq!(app.view, View::Thread);
        assert_eq!(app.selected_thread_email, Some(0));

        // Navigate in thread
        app.select_next();
        assert_eq!(app.selected_thread_email, Some(1));

        // Exit to email list
        app.exit();
        assert_eq!(app.view, View::EmailList);
        assert_eq!(app.selected_thread_email, None);

        // Exit to group list
        app.exit();
        assert_eq!(app.view, View::GroupList);
    }

    #[test]
    fn test_thread_emails() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            create_test_email_with_thread("3", "thread_b", "alice@example.com"),
        ]);

        // Select alice's group and first email
        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;
        app.enter(); // Enter email list

        // Find the email from thread_a (could be at index 0 or 1 depending on order)
        let thread_a_idx = app
            .current_group()
            .unwrap()
            .emails
            .iter()
            .position(|e| e.thread_id == "thread_a")
            .unwrap();
        app.selected_email = Some(thread_a_idx);

        // Get thread emails - should include both alice and bob's emails in thread_a
        let thread_emails = app.current_thread_emails();
        assert_eq!(thread_emails.len(), 2);

        // Verify we got the correct emails (from thread_a, not thread_b)
        let thread_ids: Vec<&str> = thread_emails.iter().map(|e| e.id.as_str()).collect();
        assert!(
            thread_ids.contains(&"1"),
            "Should contain alice's email from thread_a"
        );
        assert!(
            thread_ids.contains(&"2"),
            "Should contain bob's email from thread_a"
        );
        assert!(
            !thread_ids.contains(&"3"),
            "Should not contain alice's email from thread_b"
        );
    }

    #[test]
    fn test_thread_has_multiple_senders() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            create_test_email_with_thread("3", "thread_b", "alice@example.com"),
        ]);

        assert!(app.thread_has_multiple_senders("thread_a"));
        assert!(!app.thread_has_multiple_senders("thread_b"));
    }

    #[test]
    fn test_thread_impact_single_sender() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "alice@example.com"),
        ]);

        app.enter();
        app.selected_email = Some(0);

        let impact = app.current_email_thread_impact();
        assert!(impact.warning.is_none());
    }

    #[test]
    fn test_thread_impact_multiple_senders() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            create_test_email_with_thread("3", "thread_a", "charlie@example.com"),
        ]);

        // Select alice's group
        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;
        app.enter();
        app.selected_email = Some(0);

        let impact = app.current_email_thread_impact();
        assert_eq!(
            impact.warning,
            Some(ThreadWarning::SenderEmailMode {
                thread_count: 1,
                email_count: 2, // bob and charlie
            })
        );
    }

    #[test]
    fn test_group_thread_impact_email_mode() {
        let mut app = App::new();
        app.set_emails(vec![
            // Thread with only alice
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            // Thread with alice and bob
            create_test_email_with_thread("2", "thread_b", "alice@example.com"),
            create_test_email_with_thread("3", "thread_b", "bob@example.com"),
        ]);

        // Select alice's group (email mode is default)
        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;

        let impact = app.current_group_thread_impact();
        assert_eq!(
            impact.warning,
            Some(ThreadWarning::SenderEmailMode {
                thread_count: 1,
                email_count: 1, // bob's email
            })
        );
    }

    #[test]
    fn test_group_thread_impact_domain_mode() {
        let mut app = App::new();
        app.group_mode = GroupMode::ByDomain;
        app.set_emails(vec![
            // Thread with alice and bob (both @example.com)
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            // Single sender thread
            create_test_email_with_thread("3", "thread_b", "charlie@example.com"),
        ]);

        // Select example.com group
        let example_idx = app
            .groups
            .iter()
            .position(|g| g.key == "example.com")
            .unwrap();
        app.selected_group = example_idx;

        let impact = app.current_group_thread_impact();
        // In domain mode, we warn about multi-participant threads
        assert_eq!(
            impact.warning,
            Some(ThreadWarning::DomainMode { thread_count: 1 })
        );
    }

    #[test]
    fn test_remove_thread() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            create_test_email_with_thread("3", "thread_b", "alice@example.com"),
        ]);

        assert_eq!(app.emails.len(), 3);

        app.remove_thread("thread_a");

        assert_eq!(app.emails.len(), 1);
        assert_eq!(app.emails[0].id, "3");
    }

    #[test]
    fn test_remove_current_group_emails() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            create_test_email_with_thread("3", "thread_b", "alice@example.com"),
        ]);

        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;

        app.remove_current_group_emails();

        // Only bob's email should remain
        assert_eq!(app.emails.len(), 1);
        assert_eq!(app.emails[0].from_email, "bob@example.com");
    }

    #[test]
    fn test_remove_current_group_threads() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            create_test_email_with_thread("3", "thread_b", "charlie@example.com"),
        ]);

        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;

        app.remove_current_group_threads();

        // thread_a is removed entirely (including bob), only charlie remains
        assert_eq!(app.emails.len(), 1);
        assert_eq!(app.emails[0].from_email, "charlie@example.com");
    }

    #[test]
    fn test_groups_sorted_by_count() {
        let mut app = App::new();
        // Create bob first to verify sorting actually reorders groups
        app.set_emails(vec![
            create_test_email("1", "bob@example.com"),
            create_test_email("2", "alice@example.com"),
            create_test_email("3", "alice@example.com"),
            create_test_email("4", "alice@example.com"),
        ]);

        assert_eq!(app.groups[0].count(), 3); // alice
        assert_eq!(app.groups[1].count(), 1); // bob
    }

    #[test]
    fn test_group_thread_count() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "alice@example.com"),
            create_test_email_with_thread("3", "thread_b", "alice@example.com"),
        ]);

        let alice_group = app
            .groups
            .iter()
            .find(|g| g.key == "alice@example.com")
            .unwrap();
        assert_eq!(alice_group.count(), 3); // 3 emails
        assert_eq!(alice_group.thread_count(), 2); // 2 threads
    }

    #[test]
    fn test_current_thread_email() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
        ]);

        // Navigate to thread view
        app.enter(); // Enter email list
        app.enter(); // Enter thread view

        // Should return the first email in the thread
        let email_id = app.current_thread_email().map(|e| e.id.clone());
        assert!(email_id.is_some());

        // Navigate to second email in thread
        app.select_next();
        let email2_id = app.current_thread_email().map(|e| e.id.clone());
        assert!(email2_id.is_some());
        assert_ne!(email_id, email2_id);
    }

    #[test]
    fn test_group_has_multi_sender_threads() {
        let mut app = App::new();
        app.set_emails(vec![
            // Thread with multiple senders
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            // Thread with single sender
            create_test_email_with_thread("3", "thread_b", "charlie@example.com"),
        ]);

        let alice_group = app
            .groups
            .iter()
            .find(|g| g.key == "alice@example.com")
            .unwrap();
        assert!(app.group_has_multi_sender_threads(alice_group));

        let charlie_group = app
            .groups
            .iter()
            .find(|g| g.key == "charlie@example.com")
            .unwrap();
        assert!(!app.group_has_multi_sender_threads(charlie_group));
    }
}
