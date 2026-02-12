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
    EmailBody,
}

/// Filter for which emails/threads to display
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ThreadFilter {
    /// Show all emails (no filtering)
    #[default]
    All,
    /// Show only emails that are part of multi-message threads
    OnlyThreads,
    /// Show only emails that are NOT part of multi-message threads
    NoThreads,
}

/// Result of attempting to toggle email selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SelectionResult {
    /// Selection was toggled successfully
    Toggled,
    /// No email to select (wrong view or no current email)
    NoEmail,
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
    Group { sender: String },
    Thread { subject: String },
}

/// An entry in the undo history
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub action_type: UndoActionType,
    pub context: UndoContext,
    /// Email identifiers with their original folders: (message_id, dest_uid, original_folder)
    ///
    /// - message_id: RFC 5322 Message-ID for fallback search (None if not available)
    /// - dest_uid: UID in destination folder from COPYUID response (None if not available)
    /// - original_folder: The folder the email was in before the action
    ///
    /// When dest_uid is available, we use fast UID-based restore; otherwise fall back to Message-ID search
    pub emails: Vec<(Option<String>, Option<u32>, String)>,
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
    /// Cache of email counts per thread_id (for calculating full thread counts)
    thread_email_counts: HashMap<String, usize>,
    /// The user's email address (used to filter out sent emails from groups)
    user_email: Option<String>,
    /// Filter for which threads to display
    pub thread_filter: ThreadFilter,
    /// History of undoable actions (newest first)
    pub undo_history: Vec<UndoEntry>,
    /// Selected index in undo history view
    pub selected_undo: usize,
    /// View to return to after closing undo history
    previous_view: Option<View>,
    /// The group key we're currently viewing (to preserve view after deletions)
    viewing_group_key: Option<String>,
    /// Whether emails have been loaded at least once (to distinguish from inbox zero)
    emails_loaded: bool,
    /// Set of selected email IDs (for multi-select operations)
    selected_emails: HashSet<String>,
    /// Scroll position for text view
    pub text_view_scroll: usize,
    /// ID of the email being viewed in text view (for body caching)
    viewing_email_id: Option<String>,
    /// Active text filter query (None = no filter active)
    text_filter: Option<String>,
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
            thread_email_counts: HashMap::new(),
            user_email: None,
            thread_filter: ThreadFilter::All,
            undo_history: Vec::new(),
            selected_undo: 0,
            previous_view: None,
            viewing_group_key: None,
            emails_loaded: false,
            selected_emails: HashSet::new(),
            text_view_scroll: 0,
            viewing_email_id: None,
            text_filter: None,
        }
    }

    /// Sets the user's email address (used to filter sent emails from groups)
    pub fn set_user_email(&mut self, email: String) {
        self.user_email = Some(email);
    }

    /// Returns whether emails have been loaded at least once
    pub fn has_loaded_emails(&self) -> bool {
        self.emails_loaded
    }

    /// Sets the emails and regroups them according to current mode
    pub fn set_emails(&mut self, emails: Vec<Email>) {
        self.emails = emails;
        self.emails_loaded = true;
        self.regroup();
    }

    /// Rebuilds the cache of thread IDs with multiple messages and email counts per thread
    fn rebuild_multi_message_cache(&mut self) {
        // Count emails per thread_id
        let mut thread_counts: HashMap<&str, usize> = HashMap::new();
        for email in &self.emails {
            *thread_counts.entry(&email.thread_id).or_default() += 1;
        }

        // Store email counts per thread for full thread count calculations
        self.thread_email_counts = thread_counts
            .iter()
            .map(|(thread_id, count)| (thread_id.to_string(), *count))
            .collect();

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
        self.clear_selection();
    }

    /// Selects the next item based on current view
    pub fn select_next(&mut self) {
        match self.view {
            View::GroupList => self.select_next_group(),
            View::EmailList => self.select_next_email(),
            View::Thread => self.select_next_thread_email(),
            View::UndoHistory => self.select_next_undo(),
            View::EmailBody => self.scroll_text_view_down(1),
        }
    }

    /// Selects the previous item based on current view
    pub fn select_previous(&mut self) {
        match self.view {
            View::GroupList => self.select_previous_group(),
            View::EmailList => self.select_previous_email(),
            View::Thread => self.select_previous_thread_email(),
            View::UndoHistory => self.select_previous_undo(),
            View::EmailBody => self.scroll_text_view_up(1),
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
            View::EmailBody => {
                self.text_view_scroll = 0;
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
            View::EmailBody => {
                // Scroll to bottom - will be clamped by renderer
                self.text_view_scroll = usize::MAX;
            }
        }
    }

    /// Ensures the current selection is valid for the current view and filter settings.
    /// Call this before rendering to guarantee the selection points to a visible item.
    pub fn ensure_valid_selection(&mut self) {
        match self.view {
            View::GroupList => {
                let filtered = self.filtered_groups();
                if filtered.is_empty() {
                    return;
                }
                // Check if current selection is visible
                let current_group = self.groups.get(self.selected_group);
                let is_visible =
                    current_group.is_some_and(|g| filtered.iter().any(|fg| fg.key == g.key));
                if !is_visible {
                    // Select the first visible group
                    if let Some(first) = filtered.first() {
                        self.selected_group = self
                            .groups
                            .iter()
                            .position(|g| g.key == first.key)
                            .unwrap_or(0);
                    }
                }
            }
            View::EmailList => {
                let filtered = self.filtered_threads_in_current_group();
                if filtered.is_empty() {
                    self.selected_email = None;
                } else if self.selected_email.is_none()
                    || self.selected_email.is_some_and(|idx| idx >= filtered.len())
                {
                    self.selected_email = Some(0);
                }
            }
            View::Thread => {
                let thread_emails = self.current_thread_emails();
                if thread_emails.is_empty() {
                    self.selected_thread_email = None;
                } else if self.selected_thread_email.is_none()
                    || self
                        .selected_thread_email
                        .is_some_and(|idx| idx >= thread_emails.len())
                {
                    self.selected_thread_email = Some(0);
                }
            }
            View::UndoHistory => {
                if self.undo_history.is_empty() {
                    self.selected_undo = 0;
                } else if self.selected_undo >= self.undo_history.len() {
                    self.selected_undo = self.undo_history.len() - 1;
                }
            }
            View::EmailBody => {
                // Scroll position is managed by the renderer
            }
        }
    }

    /// Checks if a group matches the current filter
    fn group_matches_filter(&self, group: &EmailGroup) -> bool {
        match self.thread_filter {
            ThreadFilter::All => true,
            ThreadFilter::OnlyThreads => self.group_has_multi_message_threads(group),
            ThreadFilter::NoThreads => self.group_has_single_message_threads(group),
        }
    }

    /// Selects the next group in the list
    fn select_next_group(&mut self) {
        if self.thread_filter != ThreadFilter::All {
            // Find next group that matches the filter
            for i in (self.selected_group + 1)..self.groups.len() {
                if self.group_matches_filter(&self.groups[i]) {
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
        if self.thread_filter != ThreadFilter::All {
            // Find previous group that matches the filter
            for i in (0..self.selected_group).rev() {
                if self.group_matches_filter(&self.groups[i]) {
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
            View::Thread => {} // Enter handled separately in main.rs (text view)
            View::UndoHistory => {} // Enter handled separately in main.rs
            View::EmailBody => {} // Already viewing email
        }
    }

    /// Exits to the previous view level (thread -> emails -> group)
    pub fn exit(&mut self) {
        match self.view {
            View::GroupList => {} // Can't go back, handled by main.rs for quit
            View::EmailList => self.exit_to_groups(),
            View::Thread => self.exit_to_emails(),
            View::UndoHistory => self.exit_undo_history(),
            View::EmailBody => self.exit_text_view(),
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
            self.clear_selection();
        }
    }

    /// Returns to the group list view
    fn exit_to_groups(&mut self) {
        self.view = View::GroupList;
        self.selected_email = None;
        self.viewing_group_key = None;
        self.clear_selection();
        self.clear_text_filter();
    }

    /// Enters the thread view for the currently selected email
    fn enter_thread(&mut self) {
        if self.current_email().is_some() {
            self.view = View::Thread;
            self.selected_thread_email = Some(0);
            self.clear_selection();
        }
    }

    /// Returns to the email list view
    fn exit_to_emails(&mut self) {
        self.view = View::EmailList;
        self.selected_thread_email = None;
        self.clear_selection();
    }

    /// Enters the text view for viewing an email body
    pub fn enter_text_view(&mut self, email_id: &str) {
        self.previous_view = Some(self.view);
        self.viewing_email_id = Some(email_id.to_string());
        self.text_view_scroll = 0;
        self.view = View::EmailBody;
    }

    /// Exits the text view and returns to the previous view
    pub fn exit_text_view(&mut self) {
        if let Some(prev) = self.previous_view.take() {
            self.view = prev;
        } else {
            self.view = View::EmailList;
        }
        self.viewing_email_id = None;
    }

    /// Returns the email being viewed in text view
    pub fn viewing_email(&self) -> Option<&Email> {
        self.viewing_email_id
            .as_ref()
            .and_then(|id| self.emails.iter().find(|e| &e.id == id))
    }

    /// Returns the ID of the email being viewed in text view
    pub fn viewing_email_id(&self) -> Option<&str> {
        self.viewing_email_id.as_deref()
    }

    /// Sets the body of an email (caches the fetched body)
    pub fn set_email_body(&mut self, email_id: &str, body: String) {
        if let Some(email) = self.emails.iter_mut().find(|e| e.id == email_id) {
            email.body = Some(body);
        }
    }

    /// Scrolls the text view down by n lines
    pub fn scroll_text_view_down(&mut self, n: usize) {
        self.text_view_scroll = self.text_view_scroll.saturating_add(n);
    }

    /// Scrolls the text view up by n lines
    pub fn scroll_text_view_up(&mut self, n: usize) {
        self.text_view_scroll = self.text_view_scroll.saturating_sub(n);
    }

    /// Gets the currently selected email in thread view
    pub fn current_thread_email(&self) -> Option<&Email> {
        let thread_emails = self.current_thread_emails();
        self.selected_thread_email
            .and_then(|idx| thread_emails.get(idx).copied())
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
    /// Respects the filter_to_threads setting to match what's displayed in the UI.
    pub fn current_email(&self) -> Option<&Email> {
        let filtered = self.filtered_threads_in_current_group();
        self.selected_email
            .and_then(|idx| filtered.get(idx).copied())
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

    /// Checks if any email in a group is a single-message thread (not part of multi-message thread)
    pub fn group_has_single_message_threads(&self, group: &EmailGroup) -> bool {
        group
            .emails
            .iter()
            .any(|email| !self.thread_has_multiple_messages(&email.thread_id))
    }

    /// Cycles the thread filter through: All -> OnlyThreads -> NoThreads -> All
    pub fn toggle_thread_filter(&mut self) {
        self.thread_filter = match self.thread_filter {
            ThreadFilter::All => ThreadFilter::OnlyThreads,
            ThreadFilter::OnlyThreads => ThreadFilter::NoThreads,
            ThreadFilter::NoThreads => ThreadFilter::All,
        };

        if self.thread_filter != ThreadFilter::All {
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

    /// Sets the text filter query. Non-matching emails will be hidden.
    pub fn set_text_filter(&mut self, query: Option<String>) {
        self.text_filter = query;
        // Adjust selection if current email becomes hidden
        if let Some(idx) = self.selected_email {
            let filtered = self.filtered_threads_in_current_group();
            if idx >= filtered.len() {
                self.selected_email = if filtered.is_empty() { None } else { Some(0) };
            }
        }
    }

    /// Clears the text filter (shows all emails)
    pub fn clear_text_filter(&mut self) {
        self.text_filter = None;
    }

    /// Returns whether a text filter is currently active
    pub fn has_text_filter(&self) -> bool {
        self.text_filter.is_some()
    }

    /// Returns the current text filter query, if any
    pub fn text_filter(&self) -> Option<&str> {
        self.text_filter.as_deref()
    }

    /// Checks if an email matches the text filter (case-insensitive)
    fn email_matches_text_filter(&self, email: &Email) -> bool {
        let Some(ref query) = self.text_filter else {
            return true;
        };
        let query_lower = query.to_lowercase();
        email.subject.to_lowercase().contains(&query_lower)
            || email.from.to_lowercase().contains(&query_lower)
            || email.from_email.to_lowercase().contains(&query_lower)
    }

    /// Returns threads in the current group, filtered based on thread_filter and text_filter settings
    pub fn filtered_threads_in_current_group(&self) -> Vec<&Email> {
        let Some(group) = self.current_group() else {
            return Vec::new();
        };

        // First apply thread filter
        let thread_filtered: Vec<&Email> = match self.thread_filter {
            ThreadFilter::All => group.threads(),
            ThreadFilter::OnlyThreads => group
                .threads()
                .into_iter()
                .filter(|e| self.multi_message_threads.contains(&e.thread_id))
                .collect(),
            ThreadFilter::NoThreads => group
                .threads()
                .into_iter()
                .filter(|e| !self.multi_message_threads.contains(&e.thread_id))
                .collect(),
        };

        // Then apply text filter if active
        if self.text_filter.is_some() {
            thread_filtered
                .into_iter()
                .filter(|e| self.email_matches_text_filter(e))
                .collect()
        } else {
            thread_filtered
        }
    }

    /// Returns groups filtered based on thread_filter setting
    pub fn filtered_groups(&self) -> Vec<&EmailGroup> {
        match self.thread_filter {
            ThreadFilter::All => self.groups.iter().collect(),
            ThreadFilter::OnlyThreads => self
                .groups
                .iter()
                .filter(|g| self.group_has_multi_message_threads(g))
                .collect(),
            ThreadFilter::NoThreads => self
                .groups
                .iter()
                .filter(|g| self.group_has_single_message_threads(g))
                .collect(),
        }
    }

    /// Returns all emails in the current group that match the current filter.
    /// Unlike filtered_threads_in_current_group which returns one email per thread,
    /// this returns ALL emails that match (for bulk operations).
    /// Applies both thread_filter and text_filter.
    pub fn filtered_emails_in_current_group(&self) -> Vec<&Email> {
        let Some(group) = self.current_group() else {
            return Vec::new();
        };

        // First apply thread filter
        let thread_filtered: Vec<&Email> = match self.thread_filter {
            ThreadFilter::All => group.emails.iter().collect(),
            ThreadFilter::OnlyThreads => group
                .emails
                .iter()
                .filter(|e| self.multi_message_threads.contains(&e.thread_id))
                .collect(),
            ThreadFilter::NoThreads => group
                .emails
                .iter()
                .filter(|e| !self.multi_message_threads.contains(&e.thread_id))
                .collect(),
        };

        // Then apply text filter if active
        if self.text_filter.is_some() {
            thread_filtered
                .into_iter()
                .filter(|e| self.email_matches_text_filter(e))
                .collect()
        } else {
            thread_filtered
        }
    }

    /// Returns the filtered thread count for a specific group
    pub fn filtered_thread_count_for_group(&self, group: &EmailGroup) -> usize {
        match self.thread_filter {
            ThreadFilter::All => group.thread_count(),
            ThreadFilter::OnlyThreads => {
                let thread_ids: HashSet<&str> = group
                    .emails
                    .iter()
                    .filter(|e| self.multi_message_threads.contains(&e.thread_id))
                    .map(|e| e.thread_id.as_str())
                    .collect();
                thread_ids.len()
            }
            ThreadFilter::NoThreads => {
                // In NoThreads mode, each email is its own "thread" (single messages)
                group
                    .emails
                    .iter()
                    .filter(|e| !self.multi_message_threads.contains(&e.thread_id))
                    .count()
            }
        }
    }

    /// Returns the full thread email count for a group (all emails in all threads, including
    /// emails from other senders that would be shown in thread view).
    pub fn full_thread_email_count_for_group(&self, group: &EmailGroup) -> usize {
        // Get unique thread IDs in this group, filtered by current thread filter
        let thread_ids: HashSet<&str> = match self.thread_filter {
            ThreadFilter::All => group.emails.iter().map(|e| e.thread_id.as_str()).collect(),
            ThreadFilter::OnlyThreads => group
                .emails
                .iter()
                .filter(|e| self.multi_message_threads.contains(&e.thread_id))
                .map(|e| e.thread_id.as_str())
                .collect(),
            ThreadFilter::NoThreads => group
                .emails
                .iter()
                .filter(|e| !self.multi_message_threads.contains(&e.thread_id))
                .map(|e| e.thread_id.as_str())
                .collect(),
        };

        // Sum up email counts for all threads
        thread_ids
            .iter()
            .map(|tid| self.thread_email_counts.get(*tid).copied().unwrap_or(1))
            .sum()
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

    /// Removes all emails in threads that contain emails from the current group.
    /// This affects ALL emails in those threads, including from other senders.
    pub fn remove_current_group_threads(&mut self) {
        // Get thread IDs from the filtered group emails
        let thread_ids: HashSet<String> = self
            .filtered_emails_in_current_group()
            .iter()
            .map(|e| e.thread_id.clone())
            .collect();

        self.emails.retain(|e| !thread_ids.contains(&e.thread_id));
        self.regroup();
        self.selected_email = None;
        self.selected_thread_email = None;

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

    /// Gets all email IDs and source folders in the current thread
    pub fn current_thread_email_ids(&self) -> Vec<(String, String)> {
        self.current_thread_emails()
            .iter()
            .map(|e| (e.id.clone(), e.source_folder.clone()))
            .collect()
    }

    /// Gets all email IDs and source folders from threads that contain emails in the current group.
    /// This expands the operation to include ALL emails in affected threads, including from other senders.
    /// Respects the current thread filter.
    pub fn current_group_thread_email_ids(&self) -> Vec<(String, String)> {
        // Get thread IDs from the filtered group emails
        let thread_ids: HashSet<String> = self
            .filtered_emails_in_current_group()
            .iter()
            .map(|e| e.thread_id.clone())
            .collect();

        // Return all emails from those threads (including from other senders)
        self.emails
            .iter()
            .filter(|e| thread_ids.contains(&e.thread_id))
            .map(|e| (e.id.clone(), e.source_folder.clone()))
            .collect()
    }

    /// Gets all emails from threads that contain emails in the current group.
    /// This expands the operation to include ALL emails in affected threads, including from other senders.
    /// Respects the current thread filter.
    pub fn current_group_thread_emails_for_undo(&self) -> Vec<(String, Option<String>, String)> {
        // Get thread IDs from the filtered group emails
        let thread_ids: HashSet<String> = self
            .filtered_emails_in_current_group()
            .iter()
            .map(|e| e.thread_id.clone())
            .collect();

        // Return all emails from those threads (including from other senders)
        self.emails
            .iter()
            .filter(|e| thread_ids.contains(&e.thread_id))
            .map(|e| (e.id.clone(), e.message_id.clone(), e.source_folder.clone()))
            .collect()
    }

    /// Gets all email IDs and source folders from threads that contain the selected emails.
    /// This expands the operation to include ALL emails in affected threads, including from other senders.
    /// Respects the current filter - only considers selected emails that are visible.
    pub fn selected_thread_email_ids(&self) -> Vec<(String, String)> {
        // Get thread IDs from the selected emails
        let thread_ids: HashSet<String> = self
            .filtered_emails_in_current_group()
            .iter()
            .filter(|e| self.selected_emails.contains(&e.id))
            .map(|e| e.thread_id.clone())
            .collect();

        // Return all emails from those threads (including from other senders)
        self.emails
            .iter()
            .filter(|e| thread_ids.contains(&e.thread_id))
            .map(|e| (e.id.clone(), e.source_folder.clone()))
            .collect()
    }

    /// Gets all emails from threads that contain the selected emails.
    /// This expands the operation to include ALL emails in affected threads, including from other senders.
    /// Respects the current filter - only considers selected emails that are visible.
    pub fn selected_thread_emails_for_undo(&self) -> Vec<(String, Option<String>, String)> {
        // Get thread IDs from the selected emails
        let thread_ids: HashSet<String> = self
            .filtered_emails_in_current_group()
            .iter()
            .filter(|e| self.selected_emails.contains(&e.id))
            .map(|e| e.thread_id.clone())
            .collect();

        // Return all emails from those threads (including from other senders)
        self.emails
            .iter()
            .filter(|e| thread_ids.contains(&e.thread_id))
            .map(|e| (e.id.clone(), e.message_id.clone(), e.source_folder.clone()))
            .collect()
    }

    /// Gets clones of all emails from threads that contain emails in the current group.
    /// This expands the operation to include ALL emails in affected threads, including from other senders.
    /// For use in demo mode.
    pub fn current_group_thread_emails_cloned(&self) -> Vec<Email> {
        // Get thread IDs from the filtered group emails
        let thread_ids: HashSet<String> = self
            .filtered_emails_in_current_group()
            .iter()
            .map(|e| e.thread_id.clone())
            .collect();

        // Return all emails from those threads (including from other senders)
        self.emails
            .iter()
            .filter(|e| thread_ids.contains(&e.thread_id))
            .cloned()
            .collect()
    }

    /// Gets clones of all emails from threads that contain the selected emails.
    /// This expands the operation to include ALL emails in affected threads, including from other senders.
    /// For use in demo mode.
    pub fn selected_thread_emails_cloned(&self) -> Vec<Email> {
        // Get thread IDs from the selected emails
        let thread_ids: HashSet<String> = self
            .filtered_emails_in_current_group()
            .iter()
            .filter(|e| self.selected_emails.contains(&e.id))
            .map(|e| e.thread_id.clone())
            .collect();

        // Return all emails from those threads (including from other senders)
        self.emails
            .iter()
            .filter(|e| thread_ids.contains(&e.thread_id))
            .cloned()
            .collect()
    }

    /// Toggles selection of the currently highlighted email in EmailList view.
    /// Returns the result of the toggle attempt.
    pub fn toggle_email_selection(&mut self) -> SelectionResult {
        if self.view != View::EmailList {
            return SelectionResult::NoEmail;
        }

        let Some(email) = self.current_email() else {
            return SelectionResult::NoEmail;
        };

        let email_id = email.id.clone();
        if self.is_email_selected(&email_id) {
            self.selected_emails.remove(&email_id);
        } else {
            self.selected_emails.insert(email_id);
        }
        SelectionResult::Toggled
    }

    /// Clears all selected emails
    pub fn clear_selection(&mut self) {
        self.selected_emails.clear();
    }

    /// Removes specific emails from selection (for filtered operations)
    pub fn deselect_emails(&mut self, ids: &[String]) {
        for id in ids {
            self.selected_emails.remove(id);
        }
    }

    /// Returns whether a specific email is selected
    pub fn is_email_selected(&self, email_id: &str) -> bool {
        self.selected_emails.contains(email_id)
    }

    /// Returns the number of selected emails
    #[cfg(test)]
    pub fn selected_email_count(&self) -> usize {
        self.selected_emails.len()
    }

    /// Returns whether any emails are selected
    pub fn has_selection(&self) -> bool {
        !self.selected_emails.is_empty()
    }

    /// Returns whether any selected emails are visible in the current filtered view.
    /// Returns false when all selections are hidden by the text filter, or when there are no selections.
    #[allow(dead_code)] // Used in func-4: handle_delete/handle_archive will call this
    pub fn has_visible_selection(&self) -> bool {
        let visible_ids: HashSet<&str> = self
            .filtered_emails_in_current_group()
            .iter()
            .map(|e| e.id.as_str())
            .collect();

        self.selected_emails
            .iter()
            .any(|id| visible_ids.contains(id.as_str()))
    }

    /// Returns the current thread's emails' data for undo support: (uid, message_id, source_folder)
    pub fn current_thread_emails_for_undo(&self) -> Vec<(String, Option<String>, String)> {
        self.current_thread_emails()
            .iter()
            .map(|e| (e.id.clone(), e.message_id.clone(), e.source_folder.clone()))
            .collect()
    }

    /// Removes all emails in threads that contain selected emails.
    /// This affects ALL emails in those threads, including from other senders.
    pub fn remove_selected_threads(&mut self) {
        // Get thread IDs from the selected emails
        let thread_ids: HashSet<String> = self
            .filtered_emails_in_current_group()
            .iter()
            .filter(|e| self.selected_emails.contains(&e.id))
            .map(|e| e.thread_id.clone())
            .collect();

        // Remove all emails from those threads, tracking which IDs are removed
        let mut removed_ids: HashSet<String> = HashSet::new();
        self.emails.retain(|e| {
            if thread_ids.contains(&e.thread_id) {
                removed_ids.insert(e.id.clone());
                false
            } else {
                true
            }
        });

        // Only clear selections for emails that were actually removed
        self.selected_emails.retain(|id| !removed_ids.contains(id));

        self.regroup();
        self.selected_email = None;
        self.selected_thread_email = None;

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
        assert!(!app.has_loaded_emails());
    }

    #[test]
    fn test_has_loaded_emails_set_after_set_emails() {
        let mut app = App::new();
        assert!(!app.has_loaded_emails());

        // Even setting empty emails should mark as loaded
        app.set_emails(vec![]);
        assert!(app.has_loaded_emails());
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
    fn test_remove_group_threads_selects_first_in_new_group() {
        // Regression test: when deleting all threads from a group causes a switch
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

        // Delete all threads touched by alice's group
        app.remove_current_group_threads();

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
        assert_eq!(app.thread_filter, ThreadFilter::All);

        // Without filter, should see 2 threads
        assert_eq!(app.filtered_threads_in_current_group().len(), 2);

        // Toggle to OnlyThreads
        app.toggle_thread_filter();
        assert_eq!(app.thread_filter, ThreadFilter::OnlyThreads);

        // With OnlyThreads filter, should only see 1 thread (thread_a with multiple messages)
        assert_eq!(app.filtered_threads_in_current_group().len(), 1);

        // Toggle to NoThreads
        app.toggle_thread_filter();
        assert_eq!(app.thread_filter, ThreadFilter::NoThreads);

        // With NoThreads filter, should only see 1 thread (thread_b with single message)
        assert_eq!(app.filtered_threads_in_current_group().len(), 1);

        // Toggle back to All
        app.toggle_thread_filter();
        assert_eq!(app.thread_filter, ThreadFilter::All);
        assert_eq!(app.filtered_threads_in_current_group().len(), 2);
    }

    #[test]
    fn test_current_email_respects_thread_filter() {
        // Regression test: current_email() must return the email at the selected
        // index in the FILTERED list, not the unfiltered list. Otherwise, pressing
        // Enter on what looks like a multi-message thread opens the browser instead
        // of expanding the thread view.
        let mut app = App::new();
        app.set_emails(vec![
            // Single-message thread (will be filtered out)
            create_test_email_with_thread("1", "thread_single_1", "alice@example.com"),
            // Multi-message thread
            create_test_email_with_thread("2", "thread_multi_1", "alice@example.com"),
            create_test_email_with_thread("3", "thread_multi_1", "alice@example.com"),
            // Another single-message thread (will be filtered out)
            create_test_email_with_thread("4", "thread_single_2", "alice@example.com"),
            // Another multi-message thread
            create_test_email_with_thread("5", "thread_multi_2", "alice@example.com"),
            create_test_email_with_thread("6", "thread_multi_2", "alice@example.com"),
        ]);

        app.enter(); // Enter email list for alice's group

        // Verify unfiltered state: 4 threads total
        assert_eq!(app.filtered_threads_in_current_group().len(), 4);

        // Enable thread filter (OnlyThreads)
        app.toggle_thread_filter();
        assert_eq!(app.thread_filter, ThreadFilter::OnlyThreads);

        // With filter, should only see 2 threads (the multi-message ones)
        assert_eq!(app.filtered_threads_in_current_group().len(), 2);

        // Collect thread_ids from the filtered list for comparison
        let filtered_thread_ids: Vec<String> = app
            .filtered_threads_in_current_group()
            .iter()
            .map(|e| e.thread_id.clone())
            .collect();

        // Verify that every position in the filtered list returns a multi-message thread
        // This is the key invariant: what you see (filtered) is what you get (current_email)
        for (idx, expected_thread_id) in filtered_thread_ids.iter().enumerate() {
            app.selected_email = Some(idx);
            let current = app.current_email().expect("should have current email");

            // The current email must be from a multi-message thread
            assert!(
                app.thread_has_multiple_messages(&current.thread_id),
                "index {} should return a multi-message thread, got {}",
                idx,
                current.thread_id
            );

            // And it must match what's displayed at that position
            assert_eq!(
                &current.thread_id, expected_thread_id,
                "current_email() at index {} should match filtered list",
                idx
            );
        }
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
            context: UndoContext::Thread {
                subject: "Test Email".to_string(),
            },
            emails: vec![(
                Some("<1@example.com>".to_string()),
                Some(100),
                "INBOX".to_string(),
            )],
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
                (
                    Some("<2@example.com>".to_string()),
                    Some(101),
                    "INBOX".to_string(),
                ),
                (
                    Some("<3@example.com>".to_string()),
                    Some(102),
                    "INBOX".to_string(),
                ),
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
                context: UndoContext::Thread {
                    subject: format!("Email {}", i),
                },
                emails: vec![(
                    Some(format!("<{}@example.com>", i)),
                    Some(i as u32),
                    "INBOX".to_string(),
                )],
                current_folder: "[Gmail]/All Mail".to_string(),
            };
            app.push_undo(entry);
        }

        // Should be capped at MAX_UNDO_HISTORY (50)
        assert_eq!(app.undo_history_len(), 50);

        // The newest entry (59) should be at index 0
        let current = app.current_undo_entry().unwrap();
        if let UndoContext::Thread { subject } = &current.context {
            assert_eq!(subject, "Email 59");
        } else {
            panic!("Expected Thread context");
        }
    }

    #[test]
    fn test_undo_history_navigation() {
        let mut app = App::new();

        // Push some entries
        for i in 0..5 {
            let entry = UndoEntry {
                action_type: UndoActionType::Archive,
                context: UndoContext::Thread {
                    subject: format!("Email {}", i),
                },
                emails: vec![(
                    Some(format!("<{}@example.com>", i)),
                    Some(i as u32),
                    "INBOX".to_string(),
                )],
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
        let group = UndoContext::Group {
            sender: "test@example.com".to_string(),
        };
        assert!(matches!(group, UndoContext::Group { .. }));

        let thread = UndoContext::Thread {
            subject: "Thread Subject".to_string(),
        };
        assert!(matches!(thread, UndoContext::Thread { .. }));
    }

    #[test]
    fn test_ensure_valid_selection_snaps_to_visible_group() {
        // Regression test: ensure_valid_selection should snap selection to a
        // visible group when the current selection is filtered out
        let mut app = App::new();
        app.set_emails(vec![
            // Multi-message thread for alice
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "alice@example.com"),
            // Single-message emails for bob and charlie (no threads)
            create_test_email_with_thread("3", "thread_b", "bob@example.com"),
            create_test_email_with_thread("4", "thread_c", "charlie@example.com"),
        ]);

        // Enable thread filter - only alice should be visible
        app.toggle_thread_filter();
        assert_eq!(app.filtered_groups().len(), 1);

        // Manually set selected_group to bob's index (simulating what happens
        // after deleting alice's emails - the app advances to next group)
        let bob_idx = app
            .groups
            .iter()
            .position(|g| g.key == "bob@example.com")
            .unwrap();
        app.selected_group = bob_idx;

        // Before ensure_valid_selection, selection points to hidden group
        assert_eq!(app.current_group().unwrap().key, "bob@example.com");

        // After ensure_valid_selection, selection should snap to visible group
        app.ensure_valid_selection();
        assert_eq!(
            app.current_group().unwrap().key,
            "alice@example.com",
            "Selection should snap to visible group"
        );
    }

    #[test]
    fn test_ensure_valid_selection_clamps_email_index() {
        // Test that ensure_valid_selection clamps email selection when out of bounds
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_b", "alice@example.com"),
        ]);

        app.enter(); // Enter email list
        app.selected_email = Some(10); // Set to invalid index

        app.ensure_valid_selection();
        assert_eq!(
            app.selected_email,
            Some(0),
            "Selection should snap to first email when out of bounds"
        );
    }

    #[test]
    fn test_toggle_email_selection() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
        ]);

        // Must be in EmailList view to toggle selection
        app.enter();
        assert_eq!(app.view, View::EmailList);

        // Initially no selections
        assert_eq!(app.selected_email_count(), 0);
        assert!(!app.has_selection());

        // Get the current email's ID
        let first_email_id = app.current_email().unwrap().id.clone();

        // Toggle selection on first email
        assert_eq!(app.toggle_email_selection(), SelectionResult::Toggled);
        assert!(app.is_email_selected(&first_email_id));
        assert_eq!(app.selected_email_count(), 1);

        // Move to second email and get its ID
        app.select_next();
        let second_email_id = app.current_email().unwrap().id.clone();
        assert_ne!(first_email_id, second_email_id);

        // Toggle selection on second email
        assert_eq!(app.toggle_email_selection(), SelectionResult::Toggled);
        assert!(app.is_email_selected(&first_email_id));
        assert!(app.is_email_selected(&second_email_id));
        assert_eq!(app.selected_email_count(), 2);

        // Toggle again to deselect
        assert_eq!(app.toggle_email_selection(), SelectionResult::Toggled);
        assert!(app.is_email_selected(&first_email_id));
        assert!(!app.is_email_selected(&second_email_id));
        assert_eq!(app.selected_email_count(), 1);
    }

    #[test]
    fn test_toggle_email_selection_only_works_in_email_list_view() {
        let mut app = App::new();
        app.set_emails(vec![create_test_email("1", "alice@example.com")]);

        // In GroupList view, toggle should return NoEmail
        assert_eq!(app.view, View::GroupList);
        assert_eq!(app.toggle_email_selection(), SelectionResult::NoEmail);
        assert_eq!(app.selected_email_count(), 0);

        // Enter email list
        app.enter();
        assert_eq!(app.view, View::EmailList);

        // Now toggle should work
        assert_eq!(app.toggle_email_selection(), SelectionResult::Toggled);
        assert_eq!(app.selected_email_count(), 1);
    }

    #[test]
    fn test_toggle_email_selection_allows_threads() {
        let mut app = App::new();
        // Create a multi-message thread
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
        ]);

        app.enter();
        assert_eq!(app.view, View::EmailList);

        // Selecting an email that's part of a thread should work
        assert_eq!(app.toggle_email_selection(), SelectionResult::Toggled);
        assert_eq!(app.selected_email_count(), 1);
    }

    #[test]
    fn test_selection_cleared_on_view_change() {
        let mut app = App::new();
        // Use separate threads so emails can be selected
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
        ]);

        // Enter email list and select an email
        app.enter();
        assert_eq!(app.toggle_email_selection(), SelectionResult::Toggled);
        assert_eq!(app.selected_email_count(), 1);

        // Enter thread view - selection should be cleared
        app.enter();
        assert_eq!(app.view, View::Thread);
        assert_eq!(app.selected_email_count(), 0);

        // Exit to email list and select again
        app.exit();
        assert_eq!(app.view, View::EmailList);
        assert_eq!(app.toggle_email_selection(), SelectionResult::Toggled);
        assert_eq!(app.selected_email_count(), 1);

        // Exit to group list - selection should be cleared
        app.exit();
        assert_eq!(app.view, View::GroupList);
        assert_eq!(app.selected_email_count(), 0);
    }

    #[test]
    fn test_selection_cleared_on_group_mode_toggle() {
        let mut app = App::new();
        app.set_emails(vec![create_test_email("1", "alice@example.com")]);

        // Enter email list and select
        app.enter();
        app.toggle_email_selection();
        assert_eq!(app.selected_email_count(), 1);

        // Exit to group list
        app.exit();

        // Select again
        app.enter();
        app.toggle_email_selection();
        assert_eq!(app.selected_email_count(), 1);

        // Exit and toggle group mode
        app.exit();
        app.toggle_group_mode();
        assert_eq!(app.selected_email_count(), 0);
    }

    fn create_test_email_with_subject(id: &str, from: &str, subject: &str) -> Email {
        Email::new(
            id.to_string(),
            format!("thread_{id}"),
            from.to_string(),
            subject.to_string(),
            "Snippet".to_string(),
            Utc::now(),
        )
    }

    #[test]
    fn test_text_filter_hides_non_matching_emails() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_subject("1", "alice@example.com", "Hello World"),
            create_test_email_with_subject("2", "alice@example.com", "Goodbye Moon"),
            create_test_email_with_subject("3", "alice@example.com", "Hello Again"),
        ]);

        app.enter(); // Enter email list
        assert_eq!(app.view, View::EmailList);
        assert_eq!(app.filtered_threads_in_current_group().len(), 3);

        // Apply filter for "hello"
        app.set_text_filter(Some("hello".to_string()));
        let filtered = app.filtered_threads_in_current_group();
        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .iter()
                .all(|e| e.subject.to_lowercase().contains("hello"))
        );

        // Clear filter shows all emails again
        app.clear_text_filter();
        assert_eq!(app.filtered_threads_in_current_group().len(), 3);
    }

    #[test]
    fn test_text_filter_is_case_insensitive() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_subject("1", "alice@example.com", "HELLO World"),
            create_test_email_with_subject("2", "alice@example.com", "Goodbye Moon"),
        ]);

        app.enter();
        app.set_text_filter(Some("hello".to_string()));
        let filtered = app.filtered_threads_in_current_group();
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].subject.contains("HELLO"));
    }

    #[test]
    fn test_text_filter_matches_sender() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_subject("1", "alice@example.com", "Subject One"),
            create_test_email_with_subject("2", "alice@example.com", "Subject Two"),
        ]);

        app.enter();
        // Filter by partial sender name (which appears in from field)
        app.set_text_filter(Some("alice".to_string()));
        // Both should match since they're from alice
        assert_eq!(app.filtered_threads_in_current_group().len(), 2);

        // Filter by something that doesn't match
        app.set_text_filter(Some("bob".to_string()));
        assert_eq!(app.filtered_threads_in_current_group().len(), 0);
    }

    #[test]
    fn test_text_filter_adjusts_selection() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_subject("1", "alice@example.com", "First"),
            create_test_email_with_subject("2", "alice@example.com", "Second"),
            create_test_email_with_subject("3", "alice@example.com", "Third"),
        ]);

        app.enter();
        app.selected_email = Some(2); // Select the third email

        // Apply filter that hides the selected email
        app.set_text_filter(Some("First".to_string()));

        // Selection should be adjusted to first visible
        assert_eq!(app.selected_email, Some(0));
    }

    #[test]
    fn test_text_filter_cleared_on_exit_to_groups() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "bob@example.com"),
        ]);

        app.enter(); // Enter email list
        app.set_text_filter(Some("test".to_string()));
        assert!(app.has_text_filter());

        app.exit(); // Exit back to group list
        assert!(!app.has_text_filter());
    }

    #[test]
    fn test_current_group_thread_email_ids_includes_other_senders() {
        // When operating on a group, we should get ALL emails from affected threads,
        // including emails from other senders
        let mut app = App::new();
        app.set_emails(vec![
            // Thread shared between alice and bob
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
            // Alice's solo thread
            create_test_email_with_thread("3", "thread_b", "alice@example.com"),
            // Charlie's unrelated thread
            create_test_email_with_thread("4", "thread_c", "charlie@example.com"),
        ]);

        // Select alice's group
        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;

        // Get thread email IDs - should include bob's email from thread_a
        let thread_email_ids = app.current_group_thread_email_ids();
        let ids: Vec<&str> = thread_email_ids.iter().map(|(id, _)| id.as_str()).collect();

        // Should include alice's emails (1, 3)
        assert!(
            ids.contains(&"1"),
            "Should contain alice's email from thread_a"
        );
        assert!(
            ids.contains(&"3"),
            "Should contain alice's email from thread_b"
        );
        // Should include bob's email from the shared thread
        assert!(
            ids.contains(&"2"),
            "Should contain bob's email from thread_a (same thread as alice)"
        );
        // Should NOT include charlie's unrelated email
        assert!(
            !ids.contains(&"4"),
            "Should not contain charlie's unrelated email"
        );

        // Total should be 3 emails
        assert_eq!(thread_email_ids.len(), 3);
    }

    #[test]
    fn test_current_group_thread_email_ids_respects_filter() {
        // When a thread filter is active, only threads visible in that filter should be included
        let mut app = App::new();
        app.set_emails(vec![
            // Multi-message thread
            create_test_email_with_thread("1", "thread_multi", "alice@example.com"),
            create_test_email_with_thread("2", "thread_multi", "alice@example.com"),
            create_test_email_with_thread("3", "thread_multi", "bob@example.com"),
            // Single-message thread
            create_test_email_with_thread("4", "thread_single", "alice@example.com"),
        ]);

        // Enter alice's group
        app.enter();

        // Filter to only multi-message threads
        app.toggle_thread_filter();
        assert_eq!(app.thread_filter, ThreadFilter::OnlyThreads);

        let thread_email_ids = app.current_group_thread_email_ids();
        let ids: Vec<&str> = thread_email_ids.iter().map(|(id, _)| id.as_str()).collect();

        // Should include all emails from thread_multi (including bob's)
        assert!(ids.contains(&"1"));
        assert!(ids.contains(&"2"));
        assert!(
            ids.contains(&"3"),
            "Should include bob's email from the filtered thread"
        );
        // Should NOT include the single-message thread (filtered out)
        assert!(
            !ids.contains(&"4"),
            "Should not include email from filtered-out single thread"
        );
    }

    #[test]
    fn test_current_group_thread_email_ids_for_undo_includes_other_senders() {
        // The undo data should also include all emails from affected threads
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
        ]);

        let alice_idx = app
            .groups
            .iter()
            .position(|g| g.key == "alice@example.com")
            .unwrap();
        app.selected_group = alice_idx;

        let undo_data = app.current_group_thread_emails_for_undo();

        // Should have both alice and bob's emails for undo
        assert_eq!(undo_data.len(), 2);
    }

    #[test]
    fn test_selected_thread_email_ids_includes_other_senders() {
        // When operating on selected emails, we should get ALL emails from affected threads
        let mut app = App::new();
        app.set_emails(vec![
            // Single-message threads that can be selected
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_a", "bob@example.com"),
        ]);

        app.enter(); // Enter alice's group

        // Select alice's email (thread_a email 1)
        app.selected_emails.insert("1".to_string());

        let thread_email_ids = app.selected_thread_email_ids();
        let ids: Vec<&str> = thread_email_ids.iter().map(|(id, _)| id.as_str()).collect();

        // Should include both emails from the thread
        assert!(ids.contains(&"1"), "Should contain selected email");
        assert!(
            ids.contains(&"2"),
            "Should contain bob's email from same thread"
        );
    }

    #[test]
    fn test_remove_selected_threads_preserves_hidden_selections() {
        // When filtering hides some selected emails, removing visible selections
        // should preserve the hidden selections
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_thread("1", "thread_a", "alice@example.com"),
            create_test_email_with_thread("2", "thread_b", "alice@example.com"),
            create_test_email_with_thread("3", "thread_c", "alice@example.com"),
        ]);

        // Give the emails different subjects for filtering
        app.emails[0].subject = "Important meeting".to_string();
        app.emails[1].subject = "Urgent task".to_string();
        app.emails[2].subject = "Important update".to_string();
        app.regroup();

        app.enter(); // Enter alice's group

        // Select all three emails
        app.selected_emails.insert("1".to_string());
        app.selected_emails.insert("2".to_string());
        app.selected_emails.insert("3".to_string());

        // Apply filter that hides email 2 ("Urgent task")
        app.set_text_filter(Some("Important".to_string()));

        // Remove selected threads (should only remove 1 and 3)
        app.remove_selected_threads();

        // Email 2's selection should be preserved since it was hidden
        assert!(
            app.is_email_selected("2"),
            "Hidden selection should be preserved"
        );
        assert!(
            !app.is_email_selected("1"),
            "Deleted email should not be selected"
        );
        assert!(
            !app.is_email_selected("3"),
            "Deleted email should not be selected"
        );
    }

    #[test]
    fn test_has_visible_selection_false_with_no_selections() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
        ]);
        app.enter();

        assert!(!app.has_visible_selection());
    }

    #[test]
    fn test_has_visible_selection_true_with_visible_selections() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email("1", "alice@example.com"),
            create_test_email("2", "alice@example.com"),
        ]);
        app.enter();

        app.selected_emails.insert("1".to_string());
        assert!(app.has_visible_selection());
    }

    #[test]
    fn test_has_visible_selection_false_when_all_selections_hidden() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_subject("1", "alice@example.com", "Important meeting"),
            create_test_email_with_subject("2", "alice@example.com", "Urgent task"),
        ]);
        app.enter();

        // Select email 2 which has "Urgent" in subject
        app.selected_emails.insert("2".to_string());

        // Filter to only show "Important" — hides email 2
        app.set_text_filter(Some("Important".to_string()));

        assert!(app.has_selection(), "Selection still exists");
        assert!(
            !app.has_visible_selection(),
            "No visible selection since email 2 is hidden"
        );
    }

    #[test]
    fn test_has_visible_selection_true_when_some_visible_some_hidden() {
        let mut app = App::new();
        app.set_emails(vec![
            create_test_email_with_subject("1", "alice@example.com", "Important meeting"),
            create_test_email_with_subject("2", "alice@example.com", "Urgent task"),
            create_test_email_with_subject("3", "alice@example.com", "Important update"),
        ]);
        app.enter();

        // Select emails 1 and 2
        app.selected_emails.insert("1".to_string());
        app.selected_emails.insert("2".to_string());

        // Filter to only show "Important" — hides email 2 but email 1 is still visible
        app.set_text_filter(Some("Important".to_string()));

        assert!(
            app.has_visible_selection(),
            "Should be true because email 1 is visible and selected"
        );
    }
}
