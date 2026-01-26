use crate::email::Email;
use std::collections::{HashMap, HashSet};

/// Maximum number of undo entries to keep in history
const MAX_UNDO_HISTORY: usize = 50;

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
    UndoHistory,
}

/// Type of action that can be undone
#[derive(Debug, Clone, PartialEq)]
pub enum UndoActionType {
    Archive,
    Delete,
}

/// Context about what was affected by the action
#[derive(Debug, Clone, PartialEq)]
pub enum UndoContext {
    SingleEmail { subject: String },
    Group { sender: String },
    Thread { subject: String },
}

/// An entry in the undo history
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub action_type: UndoActionType,
    pub context: UndoContext,
    /// Email Message-IDs with their original folders: (message_id, original_folder)
    /// We use Message-ID instead of UID because UIDs change when emails move folders
    pub emails: Vec<(String, String)>,
    /// Where the emails are now: "[Gmail]/All Mail" or "[Gmail]/Trash"
    pub current_folder: String,
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
        self.threads().len()
    }

    /// Returns the newest email from each thread in this group.
    /// Since emails are sorted by date descending, we take the first email for each thread_id.
    pub fn threads(&self) -> Vec<&Email> {
        let mut seen_threads = HashSet::new();
        self.emails
            .iter()
            .filter(|email| seen_threads.insert(&email.thread_id))
            .collect()
    }
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
    /// Cache of thread IDs that have multiple messages (for O(1) lookup)
    multi_message_threads: HashSet<String>,
    /// The user's email address (used to filter out sent emails from groups)
    user_email: Option<String>,
    /// When true, only show emails that are part of multi-message threads
    pub filter_to_threads: bool,
    /// History of undoable actions (newest first)
    pub undo_history: Vec<UndoEntry>,
    /// Selected index in undo history view
    pub selected_undo: usize,
    /// View to return to after closing undo history
    previous_view: Option<View>,
    /// The group key we're currently viewing (to preserve view after deletions)
    viewing_group_key: Option<String>,
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
            multi_message_threads: HashSet::new(),
            user_email: None,
            filter_to_threads: false,
            undo_history: Vec::new(),
            selected_undo: 0,
            previous_view: None,
            viewing_group_key: None,
        }
    }

    /// Sets the user's email address (used to filter sent emails from groups)
    pub fn set_user_email(&mut self, email: String) {
        self.user_email = Some(email);
    }

    /// Sets the emails and regroups them according to current mode
    pub fn set_emails(&mut self, emails: Vec<Email>) {
        self.emails = emails;
        self.regroup();
    }

    /// Rebuilds the cache of thread IDs with multiple messages
    fn rebuild_multi_message_cache(&mut self) {
        // Count emails per thread_id
        let mut thread_counts: HashMap<&str, usize> = HashMap::new();
        for email in &self.emails {
            *thread_counts.entry(&email.thread_id).or_default() += 1;
        }

        // Collect thread IDs with more than one email
        self.multi_message_threads = thread_counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(thread_id, _)| thread_id.to_string())
            .collect();
    }

    /// Regroups emails according to the current group mode
    fn regroup(&mut self) {
        self.rebuild_multi_message_cache();
        let mut group_map: HashMap<String, Vec<Email>> = HashMap::new();

        for email in &self.emails {
            // Skip user's own sent emails from grouping
            // They remain in self.emails for thread view and operations
            if let Some(ref user_email) = self.user_email
                && email.from_email.eq_ignore_ascii_case(user_email)
            {
                continue;
            }

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

        // Sort groups by email count (descending), then alphabetically (ascending) as tie-breaker
        self.groups
            .sort_by_key(|g| (std::cmp::Reverse(g.count()), g.key.to_lowercase()));

        // If we're viewing a specific group, find its new index after sorting
        if let Some(ref key) = self.viewing_group_key.clone() {
            if let Some(idx) = self.groups.iter().position(|g| &g.key == key) {
                self.selected_group = idx;
            } else {
                // Group no longer exists - update viewing_group_key to current selection
                // This prevents undo from switching back to a previously deleted group
                if self.selected_group >= self.groups.len() && !self.groups.is_empty() {
                    self.selected_group = self.groups.len() - 1;
                }
                self.viewing_group_key =
                    self.groups.get(self.selected_group).map(|g| g.key.clone());
            }
        } else if self.selected_group >= self.groups.len() && !self.groups.is_empty() {
            // Reset selection if out of bounds (only when not viewing a specific group)
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
            View::UndoHistory => self.select_next_undo(),
        }
    }

    /// Selects the previous item based on current view
    pub fn select_previous(&mut self) {
        match self.view {
            View::GroupList => self.select_previous_group(),
            View::EmailList => self.select_previous_email(),
            View::Thread => self.select_previous_thread_email(),
            View::UndoHistory => self.select_previous_undo(),
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

    /// Selects the first item in the current view
    pub fn select_first(&mut self) {
        match self.view {
            View::GroupList => {
                let filtered = self.filtered_groups();
                if let Some(first) = filtered.first() {
                    self.selected_group = self
                        .groups
                        .iter()
                        .position(|g| g.key == first.key)
                        .unwrap_or(0);
                }
            }
            View::EmailList => {
                let filtered = self.filtered_threads_in_current_group();
                if !filtered.is_empty() {
                    self.selected_email = Some(0);
                }
            }
            View::Thread => {
                if !self.current_thread_emails().is_empty() {
                    self.selected_thread_email = Some(0);
                }
            }
            View::UndoHistory => {
                if !self.undo_history.is_empty() {
                    self.selected_undo = 0;
                }
            }
        }
    }

    /// Selects the last item in the current view
    pub fn select_last(&mut self) {
        match self.view {
            View::GroupList => {
                let filtered = self.filtered_groups();
                if let Some(last) = filtered.last() {
                    self.selected_group = self
                        .groups
                        .iter()
                        .position(|g| g.key == last.key)
                        .unwrap_or(0);
                }
            }
            View::EmailList => {
                let filtered = self.filtered_threads_in_current_group();
                if !filtered.is_empty() {
                    self.selected_email = Some(filtered.len() - 1);
                }
            }
            View::Thread => {
                let thread_emails = self.current_thread_emails();
                if !thread_emails.is_empty() {
                    self.selected_thread_email = Some(thread_emails.len() - 1);
                }
            }
            View::UndoHistory => {
                if !self.undo_history.is_empty() {
                    self.selected_undo = self.undo_history.len() - 1;
                }
            }
        }
    }

    /// Selects the next group in the list
    fn select_next_group(&mut self) {
        if self.filter_to_threads {
            // Find next group that has multi-message threads
            for i in (self.selected_group + 1)..self.groups.len() {
                if self.group_has_multi_message_threads(&self.groups[i]) {
                    self.selected_group = i;
                    return;
                }
            }
        } else if !self.groups.is_empty() && self.selected_group < self.groups.len() - 1 {
            self.selected_group += 1;
        }
    }

    /// Selects the previous group in the list
    fn select_previous_group(&mut self) {
        if self.filter_to_threads {
            // Find previous group that has multi-message threads
            for i in (0..self.selected_group).rev() {
                if self.group_has_multi_message_threads(&self.groups[i]) {
                    self.selected_group = i;
                    return;
                }
            }
        } else if self.selected_group > 0 {
            self.selected_group -= 1;
        }
    }

    /// Selects the next thread in the current group
    fn select_next_email(&mut self) {
        let filtered = self.filtered_threads_in_current_group();
        if filtered.is_empty() {
            return;
        }

        self.selected_email = match self.selected_email {
            Some(idx) if idx < filtered.len() - 1 => Some(idx + 1),
            Some(idx) => Some(idx),
            None => Some(0),
        };
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

    /// Selects the next item in undo history
    fn select_next_undo(&mut self) {
        if !self.undo_history.is_empty() && self.selected_undo < self.undo_history.len() - 1 {
            self.selected_undo += 1;
        }
    }

    /// Selects the previous item in undo history
    fn select_previous_undo(&mut self) {
        if self.selected_undo > 0 {
            self.selected_undo -= 1;
        }
    }

    /// Enters the undo history view
    pub fn enter_undo_history(&mut self) {
        self.previous_view = Some(self.view);
        self.view = View::UndoHistory;
        self.selected_undo = 0;
    }

    /// Exits the undo history view and returns to the previous view
    pub fn exit_undo_history(&mut self) {
        if let Some(prev) = self.previous_view.take() {
            self.view = prev;
        } else {
            self.view = View::GroupList;
        }
    }

    /// Adds an entry to the undo history (at the front, newest first)
    pub fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_history.insert(0, entry);
        // Trim to max size
        if self.undo_history.len() > MAX_UNDO_HISTORY {
            self.undo_history.truncate(MAX_UNDO_HISTORY);
        }
    }

    /// Removes and returns the undo entry at the given index
    pub fn pop_undo(&mut self, index: usize) -> Option<UndoEntry> {
        if index < self.undo_history.len() {
            let entry = self.undo_history.remove(index);
            // Adjust selected_undo if needed
            if self.selected_undo >= self.undo_history.len() && !self.undo_history.is_empty() {
                self.selected_undo = self.undo_history.len() - 1;
            }
            Some(entry)
        } else {
            None
        }
    }

    /// Returns the number of entries in undo history
    #[cfg(test)]
    pub fn undo_history_len(&self) -> usize {
        self.undo_history.len()
    }

    /// Returns the currently selected undo entry, if any
    pub fn current_undo_entry(&self) -> Option<&UndoEntry> {
        self.undo_history.get(self.selected_undo)
    }

    /// Returns the view to return to after closing undo history
    pub fn previous_view(&self) -> Option<View> {
        self.previous_view
    }

    /// Enters the next view level (group -> emails -> thread)
    pub fn enter(&mut self) {
        match self.view {
            View::GroupList => self.enter_group(),
            View::EmailList => self.enter_thread(),
            View::Thread => {}      // Already at deepest level
            View::UndoHistory => {} // Enter handled separately in main.rs
        }
    }

    /// Exits to the previous view level (thread -> emails -> group)
    pub fn exit(&mut self) {
        match self.view {
            View::GroupList => {} // Can't go back, handled by main.rs for quit
            View::EmailList => self.exit_to_groups(),
            View::Thread => self.exit_to_emails(),
            View::UndoHistory => self.exit_undo_history(),
        }
    }

    /// Enters the email list view for the currently selected group
    fn enter_group(&mut self) {
        if let Some(group) = self.groups.get(self.selected_group) {
            self.viewing_group_key = Some(group.key.clone());
            self.view = View::EmailList;
            self.selected_email = if group.threads().is_empty() {
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
        self.viewing_group_key = None;
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

    /// Gets the key of the group we're currently viewing (may be empty/deleted)
    pub fn viewing_group_key(&self) -> Option<&str> {
        self.viewing_group_key.as_deref()
    }

    /// Gets the currently selected email, if any.
    /// In email list view, this returns the newest email of the selected thread.
    pub fn current_email(&self) -> Option<&Email> {
        self.current_group().and_then(|g| {
            self.selected_email
                .and_then(|idx| g.threads().get(idx).copied())
        })
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

    /// Checks if a thread has multiple messages (O(1) lookup using cache)
    pub fn thread_has_multiple_messages(&self, thread_id: &str) -> bool {
        self.multi_message_threads.contains(thread_id)
    }

    /// Checks if the currently selected email is part of a multi-message thread
    pub fn current_email_is_multi_message_thread(&self) -> bool {
        self.current_email()
            .is_some_and(|email| self.thread_has_multiple_messages(&email.thread_id))
    }

    /// Checks if any email in a group is part of a multi-message thread
    pub fn group_has_multi_message_threads(&self, group: &EmailGroup) -> bool {
        group
            .emails
            .iter()
            .any(|email| self.thread_has_multiple_messages(&email.thread_id))
    }

    /// Toggles the thread filter (only show emails in multi-message threads)
    pub fn toggle_thread_filter(&mut self) {
        self.filter_to_threads = !self.filter_to_threads;

        if self.filter_to_threads {
            // Only adjust group selection in GroupList view
            if self.view == View::GroupList {
                let filtered_groups = self.filtered_groups();
                if !filtered_groups.is_empty() {
                    let current_group = self.groups.get(self.selected_group);
                    let still_visible = current_group
                        .is_some_and(|g| filtered_groups.iter().any(|fg| fg.key == g.key));
                    if !still_visible {
                        // Find the index of the first visible group in the unfiltered list
                        if let Some(first_visible) = filtered_groups.first() {
                            self.selected_group = self
                                .groups
                                .iter()
                                .position(|g| g.key == first_visible.key)
                                .unwrap_or(0);
                        }
                    }
                }
            }

            // Reset email selection if current email is filtered out
            if let Some(idx) = self.selected_email {
                let filtered = self.filtered_threads_in_current_group();
                if idx >= filtered.len() {
                    self.selected_email = if filtered.is_empty() { None } else { Some(0) };
                }
            }
        }
    }

    /// Returns threads in the current group, filtered if filter_to_threads is active
    pub fn filtered_threads_in_current_group(&self) -> Vec<&Email> {
        let Some(group) = self.current_group() else {
            return Vec::new();
        };
        if self.filter_to_threads {
            group
                .threads()
                .into_iter()
                .filter(|e| self.multi_message_threads.contains(&e.thread_id))
                .collect()
        } else {
            group.threads()
        }
    }

    /// Returns groups filtered if filter_to_threads is active (only groups with multi-message threads)
    pub fn filtered_groups(&self) -> Vec<&EmailGroup> {
        if self.filter_to_threads {
            self.groups
                .iter()
                .filter(|g| self.group_has_multi_message_threads(g))
                .collect()
        } else {
            self.groups.iter().collect()
        }
    }

    /// Counts all emails across all threads that a group participates in
    /// (includes emails from other senders in multi-sender threads)
    pub fn total_thread_emails_for_group(&self, group: &EmailGroup) -> usize {
        let thread_ids: HashSet<&str> = group.emails.iter().map(|e| e.thread_id.as_str()).collect();
        self.emails
            .iter()
            .filter(|e| thread_ids.contains(e.thread_id.as_str()))
            .count()
    }

    /// Removes an email by ID and regroups
    pub fn remove_email(&mut self, email_id: &str) {
        self.emails.retain(|e| e.id != email_id);
        self.regroup();

        // Adjust selected_email for the (possibly changed) current group
        if let Some(group) = self.groups.get(self.selected_group) {
            let threads = group.threads();
            if threads.is_empty() {
                self.selected_email = None;
            } else if self.selected_email.is_none()
                || self.selected_email.is_some_and(|idx| idx >= threads.len())
            {
                // Ensure a valid selection exists
                self.selected_email = Some(threads.len().saturating_sub(1));
            }
        } else {
            self.selected_email = None;
        }
    }

    /// Removes all emails in a thread by thread ID
    pub fn remove_thread(&mut self, thread_id: &str) {
        self.emails.retain(|e| e.thread_id != thread_id);
        self.regroup();

        // Adjust selected_email for the (possibly changed) current group
        if let Some(group) = self.groups.get(self.selected_group) {
            let threads = group.threads();
            if threads.is_empty() {
                self.selected_email = None;
            } else if self.selected_email.is_none()
                || self.selected_email.is_some_and(|idx| idx >= threads.len())
            {
                // Ensure a valid selection exists
                self.selected_email = Some(threads.len().saturating_sub(1));
            }
        } else {
            self.selected_email = None;
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

        // If we removed the last group, adjust selection
        if self.selected_group >= self.groups.len() && !self.groups.is_empty() {
            self.selected_group = self.groups.len() - 1;
        }

        // Update selected_email based on the (possibly new) current group
        self.selected_email = self
            .groups
            .get(self.selected_group)
            .filter(|g| !g.threads().is_empty())
            .map(|_| 0);
    }

    /// Restores emails back into the app (for undo support)
    pub fn restore_emails(&mut self, emails: Vec<Email>) {
        self.emails.extend(emails);
        self.regroup();
    }

    /// Gets all email IDs and source folders in the current group
    pub fn current_group_email_ids(&self) -> Vec<(String, String)> {
        self.current_group()
            .map(|g| {
                g.emails
                    .iter()
                    .map(|e| (e.id.clone(), e.source_folder.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Gets all email IDs and source folders in the current thread
    pub fn current_thread_email_ids(&self) -> Vec<(String, String)> {
        self.current_thread_emails()
            .iter()
            .map(|e| (e.id.clone(), e.source_folder.clone()))
            .collect()
    }

    /// Gets all email Message-IDs and source folders in the current group
    /// Only returns emails that have a Message-ID
    pub fn current_group_message_ids(&self) -> Vec<(String, String)> {
        self.current_group()
            .map(|g| {
                g.emails
                    .iter()
                    .filter_map(|e| {
                        e.message_id
                            .as_ref()
                            .map(|mid| (mid.clone(), e.source_folder.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Gets all email Message-IDs and source folders in the current thread
    /// Only returns emails that have a Message-ID
    pub fn current_thread_message_ids(&self) -> Vec<(String, String)> {
        self.current_thread_emails()
            .iter()
            .filter_map(|e| {
                e.message_id
                    .as_ref()
                    .map(|mid| (mid.clone(), e.source_folder.clone()))
            })
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
    /// Gets the thread IDs for emails in the current group
    fn current_group_thread_ids(&self) -> HashSet<String> {
        self.current_group()
            .map(|g| g.emails.iter().map(|e| e.thread_id.clone()).collect())
            .unwrap_or_default()
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
    fn test_select_first_and_last_groups() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "a@test.com"),
            create_test_email("2", "b@test.com"),
            create_test_email("3", "c@test.com"),
        ]);

        assert_eq!(app.selected_group, 0);

        app.select_last();
        assert_eq!(app.selected_group, 2);

        app.select_first();
        assert_eq!(app.selected_group, 0);
    }

    #[test]
    fn test_select_first_and_last_emails() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
            create_test_email("3", "alice@example.com"),
        ]);

        app.enter();
        assert_eq!(app.selected_email, Some(0));

        app.select_last();
        assert_eq!(app.selected_email, Some(2));

        app.select_first();
        assert_eq!(app.selected_email, Some(0));
    }

    #[test]
    fn test_select_first_and_last_thread() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            create_test_email_with_thread("3", "thread_a", "charlie@example.com"),
        ]);

        // Navigate to thread view
        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;
        app.enter(); // Enter email list
        app.enter(); // Enter thread view

        assert_eq!(app.selected_thread_email, Some(0));

        app.select_last();
        assert_eq!(app.selected_thread_email, Some(2));

        app.select_first();
        assert_eq!(app.selected_thread_email, Some(0));
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
    fn test_thread_has_multiple_messages() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            create_test_email_with_thread("3", "thread_b", "alice@example.com"),
        ]);

        // thread_a has 2 messages
        assert!(app.thread_has_multiple_messages("thread_a"));
        // thread_b has only 1 message
        assert!(!app.thread_has_multiple_messages("thread_b"));
    }

    #[test]
    fn test_current_email_is_multi_message_thread_returns_true_for_threads() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
        ]);

        // Enter alice's group
        app.enter();
        assert!(app.view == View::EmailList);

        // Email is part of a multi-message thread (thread_a has 2 emails)
        // This means pressing Enter should show thread view
        assert!(app.current_email_is_multi_message_thread());
    }

    #[test]
    fn test_current_email_is_multi_message_thread_returns_false_for_single_emails() {
        let mut app = App::new();
        app.set_emails(vec![create_test_email_with_thread(
            "1",
            "thread_a",
            "alice@example.com",
        )]);

        // Enter alice's group
        app.enter();
        assert!(app.view == View::EmailList);

        // Email is NOT part of a multi-message thread (only 1 email)
        // This means pressing Enter should open directly in browser
        assert!(!app.current_email_is_multi_message_thread());
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
    fn test_remove_group_emails_selects_first_in_new_group() {
        // Regression test: when deleting all emails from a group causes a switch
        // to a different group, selected_email should be Some(0), not None
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "bob@example.com"),
        ]);

        // Enter alice's group (should be first due to alphabetical sort)
        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;
        app.enter(); // Enter EmailList view
        assert_eq!(app.view, View::EmailList);
        assert_eq!(app.selected_email, Some(0));

        // Delete all of alice's emails
        app.remove_current_group_emails();

        // Now bob's group should be selected, with first email selected
        assert_eq!(app.groups.len(), 1);
        assert_eq!(app.groups[0].key, "bob@example.com");
        assert_eq!(
            app.selected_email,
            Some(0),
            "selected_email should be Some(0) after switching to new group"
        );
    }

    #[test]
    fn test_remove_email_maintains_valid_selection() {
        // Regression test: ensure selected_email is valid after email removal
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
        ]);

        app.enter(); // Enter EmailList view
        app.select_next(); // Select second email
        assert_eq!(app.selected_email, Some(1));

        // Remove one email, selection should adjust to remain valid
        app.remove_email("2");
        assert_eq!(app.selected_email, Some(0));
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
    fn test_groups_sorted_alphabetically_when_count_equal() {
        let mut app = App::new();
        // Create groups with equal counts, in non-alphabetical order
        app.set_emails(vec![
            create_test_email("1", "zara@example.com"),
            create_test_email("2", "bob@example.com"),
            create_test_email("3", "Alice@example.com"), // uppercase to test case-insensitivity
        ]);

        // All have count 1, should be sorted alphabetically (case-insensitive)
        assert_eq!(app.groups[0].key, "Alice@example.com");
        assert_eq!(app.groups[1].key, "bob@example.com");
        assert_eq!(app.groups[2].key, "zara@example.com");
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
    fn test_group_has_multi_message_threads() {
        let mut app = App::new();
        app.set_emails(vec![
            // Thread with multiple messages
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            // Thread with single message
            create_test_email_with_thread("3", "thread_b", "charlie@example.com"),
        ]);

        let alice_group = app
            .groups
            .iter()
            .find(|g| g.key == "alice@example.com")
            .unwrap();
        assert!(app.group_has_multi_message_threads(alice_group));

        let charlie_group = app
            .groups
            .iter()
            .find(|g| g.key == "charlie@example.com")
            .unwrap();
        assert!(!app.group_has_multi_message_threads(charlie_group));
    }

    #[test]
    fn test_toggle_thread_filter() {
        let mut app = App::new();
        app.set_emails(vec![
            // Multi-message thread
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "alice@example.com"),
            // Single-message thread
            create_test_email_with_thread("3", "thread_b", "alice@example.com"),
        ]);

        app.enter(); // Enter email list
        assert!(!app.filter_to_threads);

        // Without filter, should see 2 threads
        assert_eq!(app.filtered_threads_in_current_group().len(), 2);

        // Toggle filter on
        app.toggle_thread_filter();
        assert!(app.filter_to_threads);

        // With filter, should only see 1 thread (thread_a with multiple messages)
        assert_eq!(app.filtered_threads_in_current_group().len(), 1);

        // Toggle filter off
        app.toggle_thread_filter();
        assert!(!app.filter_to_threads);
        assert_eq!(app.filtered_threads_in_current_group().len(), 2);
    }

    #[test]
    fn test_filtered_groups() {
        let mut app = App::new();
        app.set_emails(vec![
            // Multi-message thread for alice
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "alice@example.com"),
            // Single-message thread for bob
            create_test_email_with_thread("3", "thread_b", "bob@example.com"),
            // Single-message thread for charlie
            create_test_email_with_thread("4", "thread_c", "charlie@example.com"),
        ]);

        // Without filter, should see all 3 groups
        assert_eq!(app.filtered_groups().len(), 3);

        // Toggle filter on
        app.toggle_thread_filter();

        // With filter, should only see alice's group (has multi-message thread)
        let filtered = app.filtered_groups();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].key, "alice@example.com");
    }

    #[test]
    fn test_undo_history_push_and_pop() {
        let mut app = App::new();
        assert_eq!(app.undo_history_len(), 0);

        // Push an entry
        let entry1 = UndoEntry {
            action_type: UndoActionType::Archive,
            context: UndoContext::SingleEmail {
                subject: "Test Email".to_string(),
            },
            emails: vec![("1".to_string(), "INBOX".to_string())],
            current_folder: "[Gmail]/All Mail".to_string(),
        };
        app.push_undo(entry1);
        assert_eq!(app.undo_history_len(), 1);

        // Push another entry
        let entry2 = UndoEntry {
            action_type: UndoActionType::Delete,
            context: UndoContext::Group {
                sender: "test@example.com".to_string(),
            },
            emails: vec![
                ("2".to_string(), "INBOX".to_string()),
                ("3".to_string(), "INBOX".to_string()),
            ],
            current_folder: "[Gmail]/Trash".to_string(),
        };
        app.push_undo(entry2);
        assert_eq!(app.undo_history_len(), 2);

        // Newest entry should be at index 0
        let current = app.current_undo_entry().unwrap();
        assert_eq!(current.action_type, UndoActionType::Delete);

        // Pop the first entry
        let popped = app.pop_undo(0).unwrap();
        assert_eq!(popped.action_type, UndoActionType::Delete);
        assert_eq!(app.undo_history_len(), 1);

        // Now the archive entry should be at index 0
        let current = app.current_undo_entry().unwrap();
        assert_eq!(current.action_type, UndoActionType::Archive);
    }

    #[test]
    fn test_undo_history_max_size() {
        let mut app = App::new();

        // Push more than MAX_UNDO_HISTORY entries
        for i in 0..60 {
            let entry = UndoEntry {
                action_type: UndoActionType::Archive,
                context: UndoContext::SingleEmail {
                    subject: format!("Email {}", i),
                },
                emails: vec![(i.to_string(), "INBOX".to_string())],
                current_folder: "[Gmail]/All Mail".to_string(),
            };
            app.push_undo(entry);
        }

        // Should be capped at MAX_UNDO_HISTORY (50)
        assert_eq!(app.undo_history_len(), 50);

        // The newest entry (59) should be at index 0
        let current = app.current_undo_entry().unwrap();
        if let UndoContext::SingleEmail { subject } = &current.context {
            assert_eq!(subject, "Email 59");
        } else {
            panic!("Expected SingleEmail context");
        }
    }

    #[test]
    fn test_undo_history_navigation() {
        let mut app = App::new();

        // Push some entries
        for i in 0..5 {
            let entry = UndoEntry {
                action_type: UndoActionType::Archive,
                context: UndoContext::SingleEmail {
                    subject: format!("Email {}", i),
                },
                emails: vec![(i.to_string(), "INBOX".to_string())],
                current_folder: "[Gmail]/All Mail".to_string(),
            };
            app.push_undo(entry);
        }

        app.enter_undo_history();
        assert_eq!(app.view, View::UndoHistory);
        assert_eq!(app.selected_undo, 0);

        // Navigate down
        app.select_next();
        assert_eq!(app.selected_undo, 1);

        app.select_next();
        assert_eq!(app.selected_undo, 2);

        // Navigate up
        app.select_previous();
        assert_eq!(app.selected_undo, 1);

        // Jump to first
        app.select_first();
        assert_eq!(app.selected_undo, 0);

        // Jump to last
        app.select_last();
        assert_eq!(app.selected_undo, 4);

        // Exit undo history
        app.exit_undo_history();
        assert_eq!(app.view, View::GroupList);
    }

    #[test]
    fn test_enter_undo_history_empty() {
        let mut app = App::new();

        // Should enter undo history view even when empty
        app.enter_undo_history();
        assert_eq!(app.view, View::UndoHistory);

        // Can exit back to previous view
        app.exit_undo_history();
        assert_eq!(app.view, View::GroupList);
    }

    #[test]
    fn test_undo_context_variants() {
        // Test all context variants
        let single = UndoContext::SingleEmail {
            subject: "Test".to_string(),
        };
        assert!(matches!(single, UndoContext::SingleEmail { .. }));

        let group = UndoContext::Group {
            sender: "test@example.com".to_string(),
        };
        assert!(matches!(group, UndoContext::Group { .. }));

        let thread = UndoContext::Thread {
            subject: "Thread Subject".to_string(),
        };
        assert!(matches!(thread, UndoContext::Thread { .. }));
    }
}
