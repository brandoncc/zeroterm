use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use imap::{ImapConnection, Session};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::email::{Email, EmailBuilder};

/// Trait for email operations - allows mocking in tests
#[cfg_attr(test, mockall::automock)]
pub trait EmailClient {
    /// Archives an email (removes from inbox)
    fn archive_email(&mut self, email_id: &str, folder: &str) -> Result<()>;

    /// Deletes an email (moves to trash)
    fn delete_email(&mut self, email_id: &str, folder: &str) -> Result<()>;

    /// Archives a batch of emails from a single folder (moves to All Mail)
    /// UIDs should be from the same folder for efficiency
    fn archive_batch(&mut self, uids: &[String], folder: &str) -> Result<()>;

    /// Deletes a batch of emails from a single folder (moves to Trash)
    /// UIDs should be from the same folder for efficiency
    fn delete_batch(&mut self, uids: &[String], folder: &str) -> Result<()>;

    /// Restores emails to their original folders by searching for them by Message-ID
    /// Takes a list of (message_id, current_folder, destination_folder) tuples
    fn restore_emails(&mut self, emails: &[(String, String, String)]) -> Result<()>;
}

/// IMAP client for Gmail access
pub struct ImapClient {
    session: Session<Box<dyn ImapConnection>>,
}

impl ImapClient {
    /// Creates a new IMAP client and connects to Gmail
    pub fn connect(email: &str, password: &str) -> Result<Self> {
        crate::debug_log!("ImapClient::connect: connecting to imap.gmail.com:993");
        let client = imap::ClientBuilder::new("imap.gmail.com", 993)
            .connect()
            .context("Failed to connect to IMAP server")?;

        crate::debug_log!("ImapClient::connect: logging in as {}", email);
        let session = client
            .login(email, password)
            .map_err(|e| anyhow::anyhow!("Login failed: {}", e.0))?;

        crate::debug_log!("ImapClient::connect: login successful");
        Ok(Self { session })
    }

    /// Parses an IMAP message into our Email struct
    fn parse_message(&self, fetch: &imap::types::Fetch, source_folder: &str) -> Option<Email> {
        let uid = fetch.uid?;
        let envelope = fetch.envelope()?;

        // Extract From
        let from = envelope.from.as_ref().and_then(|addrs| {
            addrs.first().map(|addr| {
                let name = addr
                    .name
                    .as_ref()
                    .map(|n| String::from_utf8_lossy(n).to_string());
                let mailbox = addr
                    .mailbox
                    .as_ref()
                    .map(|m| String::from_utf8_lossy(m).to_string());
                let host = addr
                    .host
                    .as_ref()
                    .map(|h| String::from_utf8_lossy(h).to_string());

                match (name, mailbox, host) {
                    (Some(n), Some(m), Some(h)) => format!("{} <{}@{}>", n, m, h),
                    (None, Some(m), Some(h)) => format!("{}@{}", m, h),
                    _ => "unknown".to_string(),
                }
            })
        })?;

        // Extract Subject
        let subject = envelope
            .subject
            .as_ref()
            .map(|s| decode_header_value(s))
            .unwrap_or_default();

        // Extract Date
        let date_str = envelope
            .date
            .as_ref()
            .map(|d| String::from_utf8_lossy(d).to_string());
        let date = date_str
            .and_then(|s| parse_email_date(&s))
            .unwrap_or_else(Utc::now);

        // Parse headers for Message-ID, In-Reply-To, and References
        let (message_id, in_reply_to, references) = fetch
            .header()
            .map(parse_threading_headers)
            .unwrap_or((None, None, Vec::new()));

        // Create snippet from first part of subject for now
        // (full body parsing would require fetching BODY[TEXT])
        let snippet: String = subject.chars().take(100).collect();

        let mut builder = EmailBuilder::new()
            .id(uid.to_string())
            .from(from)
            .subject(subject)
            .snippet(snippet)
            .date(date)
            .references(references)
            .source_folder(source_folder);

        if let Some(msg_id) = message_id {
            builder = builder.message_id(msg_id);
        }
        if let Some(reply_to) = in_reply_to {
            builder = builder.in_reply_to(reply_to);
        }

        Some(builder.build())
    }

    /// Gets the message count for a folder without fetching all messages
    pub fn get_folder_count(&mut self, folder: &str) -> Result<u32> {
        let mailbox = self
            .session
            .select(folder)
            .context(format!("Failed to select {}", folder))?;
        Ok(mailbox.exists)
    }

    /// Fetches inbox emails within a sequence range (inclusive)
    /// If a progress counter is provided, it will be incremented for each email parsed
    pub fn fetch_inbox_range(
        &mut self,
        start: u32,
        end: u32,
        progress: Option<&Arc<AtomicUsize>>,
    ) -> Result<Vec<Email>> {
        self.fetch_folder_range("INBOX", start, end, progress)
    }

    /// Fetches sent emails within a sequence range (inclusive)
    /// If a progress counter is provided, it will be incremented for each email parsed
    pub fn fetch_sent_range(
        &mut self,
        start: u32,
        end: u32,
        progress: Option<&Arc<AtomicUsize>>,
    ) -> Result<Vec<Email>> {
        self.fetch_folder_range("[Gmail]/Sent Mail", start, end, progress)
    }

    /// Fetches emails from a folder within a sequence range (inclusive)
    fn fetch_folder_range(
        &mut self,
        folder: &str,
        start: u32,
        end: u32,
        progress: Option<&Arc<AtomicUsize>>,
    ) -> Result<Vec<Email>> {
        if start > end || start == 0 {
            return Ok(Vec::new());
        }

        self.session
            .select(folder)
            .context(format!("Failed to select {}", folder))?;

        let sequence = format!("{}:{}", start, end);
        let messages = self
            .session
            .fetch(&sequence, "(UID ENVELOPE BODY.PEEK[HEADER])")
            .context(format!(
                "Failed to fetch messages from {} ({})",
                folder, sequence
            ))?;

        let mut emails = Vec::new();
        for msg in messages.iter() {
            if let Some(email) = self.parse_message(msg, folder) {
                emails.push(email);
                if let Some(counter) = progress {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        Ok(emails)
    }

    /// Logs out and closes the connection
    pub fn logout(mut self) -> Result<()> {
        self.session.logout().context("Failed to logout")?;
        Ok(())
    }
}

impl EmailClient for ImapClient {
    fn archive_email(&mut self, uid: &str, folder: &str) -> Result<()> {
        self.session
            .select(folder)
            .context(format!("Failed to select {}", folder))?;

        // Move to All Mail (Gmail's archive)
        self.session
            .uid_mv(uid, "[Gmail]/All Mail")
            .context("Failed to archive email")?;

        Ok(())
    }

    fn delete_email(&mut self, uid: &str, folder: &str) -> Result<()> {
        self.session
            .select(folder)
            .context(format!("Failed to select {}", folder))?;

        // Move to Trash
        self.session
            .uid_mv(uid, "[Gmail]/Trash")
            .context("Failed to delete email")?;

        Ok(())
    }

    fn archive_batch(&mut self, uids: &[String], folder: &str) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }

        crate::debug_log!(
            "archive_batch: archiving {} emails from '{}'",
            uids.len(),
            folder
        );

        self.session
            .select(folder)
            .context(format!("Failed to select {}", folder))?;

        let uid_sequence = uids.join(",");
        self.session
            .uid_mv(&uid_sequence, "[Gmail]/All Mail")
            .context("Failed to archive emails")?;

        crate::debug_log!("archive_batch: done");
        Ok(())
    }

    fn delete_batch(&mut self, uids: &[String], folder: &str) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }

        crate::debug_log!(
            "delete_batch: deleting {} emails from '{}'",
            uids.len(),
            folder
        );

        self.session
            .select(folder)
            .context(format!("Failed to select {}", folder))?;

        let uid_sequence = uids.join(",");
        self.session
            .uid_mv(&uid_sequence, "[Gmail]/Trash")
            .context("Failed to delete emails")?;

        crate::debug_log!("delete_batch: done");
        Ok(())
    }

    fn restore_emails(&mut self, emails: &[(String, String, String)]) -> Result<()> {
        use std::collections::HashMap;
        use std::time::Instant;

        crate::debug_log!("restore_emails: processing {} emails", emails.len());

        // Group emails by current folder to minimize folder switches
        let mut by_current_folder: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for (message_id, current_folder, dest_folder) in emails {
            by_current_folder
                .entry(current_folder.as_str())
                .or_default()
                .push((message_id.as_str(), dest_folder.as_str()));
        }

        crate::debug_log!(
            "restore_emails: grouped into {} source folders",
            by_current_folder.len()
        );

        // Process each current folder group
        for (current_folder, moves) in by_current_folder {
            crate::debug_log!(
                "restore_emails: selecting folder '{}' ({} emails to search)",
                current_folder,
                moves.len()
            );
            let select_start = Instant::now();
            self.session
                .select(current_folder)
                .context(format!("Failed to select {}", current_folder))?;
            crate::debug_log!(
                "restore_emails: folder selected in {:.3}s",
                select_start.elapsed().as_secs_f64()
            );

            // First pass: search for all Message-IDs to get their current UIDs
            // Group by destination folder for batch moves
            let mut by_dest_folder: HashMap<&str, Vec<u32>> = HashMap::new();

            let search_start = Instant::now();
            let mut found_count = 0;
            for (i, (message_id, dest_folder)) in moves.iter().enumerate() {
                let search_query = format!("HEADER Message-ID {}", message_id);
                if i % 50 == 0 {
                    crate::debug_log!(
                        "restore_emails: searching for email {}/{} ({:.2}s elapsed)",
                        i + 1,
                        moves.len(),
                        search_start.elapsed().as_secs_f64()
                    );
                }
                let uids = self
                    .session
                    .uid_search(&search_query)
                    .context(format!("Failed to search for Message-ID {}", message_id))?;

                if let Some(uid) = uids.into_iter().next() {
                    by_dest_folder.entry(dest_folder).or_default().push(uid);
                    found_count += 1;
                }
                // If email not found, it may have been permanently deleted or already moved
                // Continue with other emails rather than failing entirely
            }
            crate::debug_log!(
                "restore_emails: search complete - found {}/{} emails in {:.2}s",
                found_count,
                moves.len(),
                search_start.elapsed().as_secs_f64()
            );

            // Second pass: batch move UIDs to each destination folder
            for (dest_folder, uids) in by_dest_folder {
                if uids.is_empty() {
                    continue;
                }

                crate::debug_log!(
                    "restore_emails: moving {} emails to '{}'",
                    uids.len(),
                    dest_folder
                );
                let move_start = Instant::now();

                let uid_sequence = uids
                    .iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>()
                    .join(",");

                self.session
                    .uid_mv(&uid_sequence, dest_folder)
                    .context(format!("Failed to restore emails to {}", dest_folder))?;

                crate::debug_log!(
                    "restore_emails: move completed in {:.3}s",
                    move_start.elapsed().as_secs_f64()
                );
            }
        }

        crate::debug_log!("restore_emails: done");
        Ok(())
    }
}

/// Decodes a potentially MIME-encoded header value
fn decode_header_value(value: &[u8]) -> String {
    // Try to parse as MIME encoded-word
    let value_str = String::from_utf8_lossy(value);

    // Handle =?charset?encoding?text?= format
    if value_str.contains("=?")
        && let Ok(decoded) = mailparse::parse_header(format!("X: {}", value_str).as_bytes())
    {
        return decoded.0.get_value();
    }

    value_str.to_string()
}

/// Parses Message-ID, In-Reply-To, and References headers from raw header bytes
fn parse_threading_headers(headers: &[u8]) -> (Option<String>, Option<String>, Vec<String>) {
    let headers_str = String::from_utf8_lossy(headers);
    let mut message_id = None;
    let mut in_reply_to = None;
    let mut references = Vec::new();

    // Unfold headers (RFC 5322: folded headers have whitespace continuation)
    let unfolded = headers_str.replace("\r\n ", " ").replace("\r\n\t", " ");

    for line in unfolded.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.starts_with("message-id:") {
            message_id = Some(line[11..].trim().to_string());
        } else if line_lower.starts_with("in-reply-to:") {
            in_reply_to = Some(line[12..].trim().to_string());
        } else if line_lower.starts_with("references:") {
            // References is a space-separated list of Message-IDs
            let refs_str = line[11..].trim();
            references = parse_message_id_list(refs_str);
        }
    }

    (message_id, in_reply_to, references)
}

/// Parses a space-separated list of Message-IDs (used for References header)
fn parse_message_id_list(s: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut current = String::new();
    let mut in_angle = false;

    for c in s.chars() {
        match c {
            '<' => {
                in_angle = true;
                current.push(c);
            }
            '>' => {
                current.push(c);
                in_angle = false;
                if !current.is_empty() {
                    ids.push(current.trim().to_string());
                    current = String::new();
                }
            }
            ' ' | '\t' if !in_angle => {
                // Skip whitespace between Message-IDs
            }
            _ => {
                current.push(c);
            }
        }
    }

    // Handle any remaining content (shouldn't happen with well-formed headers)
    if !current.is_empty() {
        ids.push(current.trim().to_string());
    }

    ids
}

/// Parses an email date string into a DateTime
fn parse_email_date(date_str: &str) -> Option<DateTime<Utc>> {
    // Try RFC2822 format first (most common for email)
    if let Ok(dt) = DateTime::parse_from_rfc2822(date_str) {
        return Some(dt.with_timezone(&Utc));
    }

    // Try other common formats
    let formats = [
        "%a, %d %b %Y %H:%M:%S %z",
        "%d %b %Y %H:%M:%S %z",
        "%a, %d %b %Y %H:%M:%S %Z",
    ];

    for fmt in &formats {
        if let Ok(dt) = DateTime::parse_from_str(date_str, fmt) {
            return Some(dt.with_timezone(&Utc));
        }
    }

    // Try parsing as timestamp
    if let Ok(ts) = date_str.parse::<i64>() {
        return Utc.timestamp_opt(ts, 0).single();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_parse_email_date_rfc2822() {
        // January 25, 2026 is a Sunday
        let date = parse_email_date("Sun, 25 Jan 2026 10:30:00 -0500");
        assert!(date.is_some());
        let dt = date.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 25);
    }

    #[test]
    fn test_parse_email_date_no_day_name() {
        let date = parse_email_date("25 Jan 2026 10:30:00 +0000");
        assert!(date.is_some());
    }

    #[test]
    fn test_parse_email_date_invalid() {
        let date = parse_email_date("invalid date");
        assert!(date.is_none());
    }

    #[test]
    fn test_parse_threading_headers() {
        let headers = b"Message-ID: <abc123@example.com>\r\nIn-Reply-To: <def456@example.com>\r\n";
        let (msg_id, reply_to, refs) = parse_threading_headers(headers);
        assert_eq!(msg_id, Some("<abc123@example.com>".to_string()));
        assert_eq!(reply_to, Some("<def456@example.com>".to_string()));
        assert!(refs.is_empty());
    }

    #[test]
    fn test_parse_threading_headers_no_reply() {
        let headers = b"Message-ID: <abc123@example.com>\r\nSubject: Test\r\n";
        let (msg_id, reply_to, refs) = parse_threading_headers(headers);
        assert_eq!(msg_id, Some("<abc123@example.com>".to_string()));
        assert_eq!(reply_to, None);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_parse_threading_headers_with_references() {
        let headers = b"Message-ID: <msg3@example.com>\r\nIn-Reply-To: <msg2@example.com>\r\nReferences: <msg1@example.com> <msg2@example.com>\r\n";
        let (msg_id, reply_to, refs) = parse_threading_headers(headers);
        assert_eq!(msg_id, Some("<msg3@example.com>".to_string()));
        assert_eq!(reply_to, Some("<msg2@example.com>".to_string()));
        assert_eq!(refs, vec!["<msg1@example.com>", "<msg2@example.com>"]);
    }

    #[test]
    fn test_parse_message_id_list() {
        let list = "<msg1@example.com> <msg2@example.com> <msg3@example.com>";
        let ids = parse_message_id_list(list);
        assert_eq!(
            ids,
            vec![
                "<msg1@example.com>",
                "<msg2@example.com>",
                "<msg3@example.com>"
            ]
        );
    }

    #[test]
    fn test_parse_message_id_list_with_extra_whitespace() {
        let list = "  <msg1@example.com>   <msg2@example.com>  ";
        let ids = parse_message_id_list(list);
        assert_eq!(ids, vec!["<msg1@example.com>", "<msg2@example.com>"]);
    }

    // Mock client tests
    #[test]
    fn test_mock_client_archive() {
        let mut mock = MockEmailClient::new();

        mock.expect_archive_email()
            .with(
                mockall::predicate::eq("email123"),
                mockall::predicate::eq("INBOX"),
            )
            .returning(|_, _| Ok(()));

        let result = mock.archive_email("email123", "INBOX");
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_client_delete() {
        let mut mock = MockEmailClient::new();

        mock.expect_delete_email()
            .with(
                mockall::predicate::eq("email456"),
                mockall::predicate::eq("INBOX"),
            )
            .returning(|_, _| Ok(()));

        let result = mock.delete_email("email456", "INBOX");
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_client_restore() {
        let mut mock = MockEmailClient::new();

        mock.expect_restore_emails().returning(|_| Ok(()));

        let restore_ops = vec![
            (
                "123".to_string(),
                "[Gmail]/All Mail".to_string(),
                "INBOX".to_string(),
            ),
            (
                "456".to_string(),
                "[Gmail]/Trash".to_string(),
                "INBOX".to_string(),
            ),
        ];
        let result = mock.restore_emails(&restore_ops);
        assert!(result.is_ok());
    }
}
