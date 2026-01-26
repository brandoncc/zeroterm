use chrono::{DateTime, Utc};
use regex::Regex;
use std::collections::HashMap;

/// Represents an email message
#[derive(Debug, Clone, PartialEq)]
pub struct Email {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub from_email: String,
    pub from_domain: String,
    pub subject: String,
    pub snippet: String,
    pub date: DateTime<Utc>,
    /// The Message-ID header value
    pub message_id: Option<String>,
    /// The In-Reply-To header value (references immediate parent)
    pub in_reply_to: Option<String>,
    /// The References header (list of all Message-IDs in the conversation chain)
    pub references: Vec<String>,
    /// The IMAP folder this email came from ("INBOX" or "[Gmail]/Sent Mail")
    pub source_folder: String,
}

/// Builder for creating Email instances
#[derive(Default)]
pub struct EmailBuilder {
    id: String,
    from: String,
    subject: String,
    snippet: String,
    date: Option<DateTime<Utc>>,
    message_id: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    source_folder: String,
}

impl EmailBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn from(mut self, from: impl Into<String>) -> Self {
        self.from = from.into();
        self
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    pub fn snippet(mut self, snippet: impl Into<String>) -> Self {
        self.snippet = snippet.into();
        self
    }

    pub fn date(mut self, date: DateTime<Utc>) -> Self {
        self.date = Some(date);
        self
    }

    pub fn message_id(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    pub fn in_reply_to(mut self, in_reply_to: impl Into<String>) -> Self {
        self.in_reply_to = Some(in_reply_to.into());
        self
    }

    pub fn references(mut self, references: Vec<String>) -> Self {
        self.references = references;
        self
    }

    pub fn source_folder(mut self, source_folder: impl Into<String>) -> Self {
        self.source_folder = source_folder.into();
        self
    }

    pub fn build(self) -> Email {
        let from_email = extract_email(&self.from);
        let from_domain = extract_domain(&from_email);

        Email {
            id: self.id,
            thread_id: String::new(), // Will be set by build_thread_ids
            from: self.from,
            from_email,
            from_domain,
            subject: self.subject,
            snippet: self.snippet,
            date: self.date.unwrap_or_else(Utc::now),
            message_id: self.message_id,
            in_reply_to: self.in_reply_to,
            references: self.references,
            source_folder: if self.source_folder.is_empty() {
                "INBOX".to_string()
            } else {
                self.source_folder
            },
        }
    }
}

/// Extracts the email address from a "Name <email>" format string
/// If no angle brackets are present, returns the string trimmed as-is
pub fn extract_email(from: &str) -> String {
    let re = Regex::new(r"<([^>]+)>").unwrap();
    if let Some(captures) = re.captures(from) {
        captures
            .get(1)
            .map_or_else(String::new, |m| m.as_str().to_string())
    } else {
        from.trim().to_string()
    }
}

/// Extracts the domain from an email address.
/// If no `@` is present, returns the full email to avoid grouping unrelated
/// malformed addresses together.
pub fn extract_domain(email: &str) -> String {
    email.split('@').nth(1).unwrap_or(email).to_string()
}

/// Builds thread IDs for a collection of emails using Message-ID, In-Reply-To, and References headers.
/// Uses a union-find algorithm to group connected emails into threads.
pub fn build_thread_ids(emails: &mut [Email]) {
    if emails.is_empty() {
        return;
    }

    // Map Message-ID to email index (for emails in the inbox)
    let mut msg_id_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, email) in emails.iter().enumerate() {
        if let Some(ref msg_id) = email.message_id {
            msg_id_to_idx.insert(msg_id.clone(), i);
        }
    }

    // Track which emails reference each Message-ID (including missing emails)
    let mut reference_to_emails: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, email) in emails.iter().enumerate() {
        // Track In-Reply-To references
        if let Some(ref reply_to) = email.in_reply_to {
            reference_to_emails
                .entry(reply_to.clone())
                .or_default()
                .push(i);
        }
        // Track References
        for reference in &email.references {
            reference_to_emails
                .entry(reference.clone())
                .or_default()
                .push(i);
        }
    }

    // Union-find parent array
    let mut parent: Vec<usize> = (0..emails.len()).collect();

    // Union emails that are connected via In-Reply-To or References to emails in inbox
    for (i, email) in emails.iter().enumerate() {
        // Check In-Reply-To
        if let Some(ref reply_to) = email.in_reply_to
            && let Some(&j) = msg_id_to_idx.get(reply_to)
        {
            union(&mut parent, i, j);
        }

        // Check all References
        for reference in &email.references {
            if let Some(&j) = msg_id_to_idx.get(reference) {
                union(&mut parent, i, j);
            }
        }
    }

    // Union emails that share a common reference (even to missing emails)
    for emails_referencing in reference_to_emails.values() {
        if emails_referencing.len() > 1 {
            let first = emails_referencing[0];
            for &other in &emails_referencing[1..] {
                union(&mut parent, first, other);
            }
        }
    }

    // Assign thread IDs based on root of each component
    for (i, email) in emails.iter_mut().enumerate() {
        let root = find(&parent, i);
        email.thread_id = format!("thread_{}", root);
    }
}

/// Find operation for union-find with path compression
fn find(parent: &[usize], mut i: usize) -> usize {
    while parent[i] != i {
        i = parent[i];
    }
    i
}

/// Union operation for union-find
fn union(parent: &mut [usize], i: usize, j: usize) {
    let root_i = find(parent, i);
    let root_j = find(parent, j);
    if root_i != root_j {
        // Always use the smaller index as root for consistency
        if root_i < root_j {
            parent[root_j] = root_i;
        } else {
            parent[root_i] = root_j;
        }
    }
}

#[cfg(test)]
impl Email {
    /// Creates a new Email, automatically extracting email and domain from the from field
    pub fn new(
        id: String,
        thread_id: String,
        from: String,
        subject: String,
        snippet: String,
        date: DateTime<Utc>,
    ) -> Self {
        let from_email = extract_email(&from);
        let from_domain = extract_domain(&from_email);

        Self {
            id,
            thread_id,
            from,
            from_email,
            from_domain,
            subject,
            snippet,
            date,
            message_id: None,
            in_reply_to: None,
            references: Vec::new(),
            source_folder: "INBOX".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_email_with_name_and_brackets() {
        assert_eq!(
            extract_email("John Doe <john@example.com>"),
            "john@example.com"
        );
    }

    #[test]
    fn test_extract_email_with_only_brackets() {
        assert_eq!(extract_email("<jane@test.org>"), "jane@test.org");
    }

    #[test]
    fn test_extract_email_without_brackets() {
        assert_eq!(extract_email("plain@email.com"), "plain@email.com");
    }

    #[test]
    fn test_extract_email_with_whitespace() {
        assert_eq!(extract_email("  spaced@email.com  "), "spaced@email.com");
    }

    #[test]
    fn test_extract_email_complex_name() {
        assert_eq!(
            extract_email("\"Doe, John\" <john.doe@company.co.uk>"),
            "john.doe@company.co.uk"
        );
    }

    #[test]
    fn test_extract_domain_simple() {
        assert_eq!(extract_domain("user@example.com"), "example.com");
    }

    #[test]
    fn test_extract_domain_subdomain() {
        assert_eq!(extract_domain("user@mail.example.com"), "mail.example.com");
    }

    #[test]
    fn test_extract_domain_no_at_symbol_returns_full_input() {
        // Returns full input to avoid grouping unrelated malformed addresses
        assert_eq!(extract_domain("invalid"), "invalid");
    }

    #[test]
    fn test_email_struct_creation() {
        let date = Utc::now();
        let email = Email::new(
            "msg123".to_string(),
            "thread456".to_string(),
            "John Doe <john@example.com>".to_string(),
            "Hello World".to_string(),
            "This is a snippet...".to_string(),
            date,
        );

        assert_eq!(email.id, "msg123");
        assert_eq!(email.thread_id, "thread456");
        assert_eq!(email.from, "John Doe <john@example.com>");
        assert_eq!(email.from_email, "john@example.com");
        assert_eq!(email.from_domain, "example.com");
        assert_eq!(email.subject, "Hello World");
        assert_eq!(email.snippet, "This is a snippet...");
    }

    #[test]
    fn test_email_struct_plain_email() {
        let date = Utc::now();
        let email = Email::new(
            "msg789".to_string(),
            "thread789".to_string(),
            "noreply@service.io".to_string(),
            "Notification".to_string(),
            "You have a new message".to_string(),
            date,
        );

        assert_eq!(email.from_email, "noreply@service.io");
        assert_eq!(email.from_domain, "service.io");
    }

    #[test]
    fn test_build_thread_ids_single_email() {
        let date = Utc::now();
        let mut emails = vec![
            EmailBuilder::new()
                .id("1")
                .from("alice@example.com")
                .subject("Subject")
                .snippet("Snippet")
                .date(date)
                .message_id("<msg1@example.com>")
                .build(),
        ];

        build_thread_ids(&mut emails);
        assert_eq!(emails[0].thread_id, "thread_0");
    }

    #[test]
    fn test_build_thread_ids_reply_chain() {
        let date = Utc::now();
        let mut emails = vec![
            EmailBuilder::new()
                .id("1")
                .from("alice@example.com")
                .subject("Subject")
                .snippet("Original")
                .date(date)
                .message_id("<msg1@example.com>")
                .build(),
            EmailBuilder::new()
                .id("2")
                .from("bob@example.com")
                .subject("Re: Subject")
                .snippet("Reply")
                .date(date)
                .message_id("<msg2@example.com>")
                .in_reply_to("<msg1@example.com>")
                .build(),
        ];

        build_thread_ids(&mut emails);
        // Both should have the same thread ID
        assert_eq!(emails[0].thread_id, emails[1].thread_id);
    }

    #[test]
    fn test_build_thread_ids_separate_threads() {
        let date = Utc::now();
        let mut emails = vec![
            EmailBuilder::new()
                .id("1")
                .from("alice@example.com")
                .subject("Subject A")
                .snippet("Email A")
                .date(date)
                .message_id("<msg1@example.com>")
                .build(),
            EmailBuilder::new()
                .id("2")
                .from("bob@example.com")
                .subject("Subject B")
                .snippet("Email B")
                .date(date)
                .message_id("<msg2@example.com>")
                .build(),
        ];

        build_thread_ids(&mut emails);
        // Should have different thread IDs
        assert_ne!(emails[0].thread_id, emails[1].thread_id);
    }

    #[test]
    fn test_build_thread_ids_three_email_chain() {
        let date = Utc::now();
        let mut emails = vec![
            EmailBuilder::new()
                .id("1")
                .from("alice@example.com")
                .subject("Subject")
                .snippet("Original")
                .date(date)
                .message_id("<msg1@example.com>")
                .build(),
            EmailBuilder::new()
                .id("2")
                .from("bob@example.com")
                .subject("Re: Subject")
                .snippet("Reply 1")
                .date(date)
                .message_id("<msg2@example.com>")
                .in_reply_to("<msg1@example.com>")
                .build(),
            EmailBuilder::new()
                .id("3")
                .from("alice@example.com")
                .subject("Re: Subject")
                .snippet("Reply 2")
                .date(date)
                .message_id("<msg3@example.com>")
                .in_reply_to("<msg2@example.com>")
                .build(),
        ];

        build_thread_ids(&mut emails);
        // All three should have the same thread ID
        assert_eq!(emails[0].thread_id, emails[1].thread_id);
        assert_eq!(emails[1].thread_id, emails[2].thread_id);
    }

    #[test]
    fn test_build_thread_ids_with_references() {
        // Test that References header can connect emails even when intermediate is missing
        let date = Utc::now();
        let mut emails = vec![
            // Original email
            EmailBuilder::new()
                .id("1")
                .from("alice@example.com")
                .subject("Subject")
                .snippet("Original")
                .date(date)
                .message_id("<msg1@example.com>")
                .build(),
            // Reply 3 - intermediate reply (msg2) is missing from inbox
            // But References header contains the full chain
            EmailBuilder::new()
                .id("3")
                .from("charlie@example.com")
                .subject("Re: Subject")
                .snippet("Reply to missing email")
                .date(date)
                .message_id("<msg3@example.com>")
                .in_reply_to("<msg2@example.com>") // This won't match (msg2 not in inbox)
                .references(vec![
                    "<msg1@example.com>".to_string(),
                    "<msg2@example.com>".to_string(),
                ])
                .build(),
        ];

        build_thread_ids(&mut emails);
        // Should be in same thread because References contains msg1
        assert_eq!(emails[0].thread_id, emails[1].thread_id);
    }

    #[test]
    fn test_build_thread_ids_shared_in_reply_to_missing_email() {
        // Two emails that both reply to a common email that's not in the inbox
        let date = Utc::now();
        let mut emails = vec![
            EmailBuilder::new()
                .id("1")
                .from("alice@example.com")
                .subject("Re: Subject")
                .snippet("Reply 1")
                .date(date)
                .message_id("<reply1@example.com>")
                .in_reply_to("<original@example.com>") // Not in inbox
                .build(),
            EmailBuilder::new()
                .id("2")
                .from("bob@example.com")
                .subject("Re: Subject")
                .snippet("Reply 2")
                .date(date)
                .message_id("<reply2@example.com>")
                .in_reply_to("<original@example.com>") // Not in inbox
                .build(),
        ];

        build_thread_ids(&mut emails);
        // Should be in same thread because they share a common In-Reply-To
        assert_eq!(emails[0].thread_id, emails[1].thread_id);
    }

    #[test]
    fn test_build_thread_ids_shared_reference_to_missing_email() {
        // Two emails that both reference a common email that's not in the inbox
        let date = Utc::now();
        let mut emails = vec![
            EmailBuilder::new()
                .id("1")
                .from("alice@example.com")
                .subject("Re: Subject")
                .snippet("Reply 1")
                .date(date)
                .message_id("<reply1@example.com>")
                .references(vec!["<original@example.com>".to_string()])
                .build(),
            EmailBuilder::new()
                .id("2")
                .from("bob@example.com")
                .subject("Re: Subject")
                .snippet("Reply 2")
                .date(date)
                .message_id("<reply2@example.com>")
                .references(vec!["<original@example.com>".to_string()])
                .build(),
        ];

        build_thread_ids(&mut emails);
        // Should be in same thread because they share a common reference
        assert_eq!(emails[0].thread_id, emails[1].thread_id);
    }
}
