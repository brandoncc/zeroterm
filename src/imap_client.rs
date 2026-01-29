use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use imap::{ImapConnection, Session};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::email::{Email, EmailBuilder};

use std::collections::HashMap;

/// Trait for email operations - allows mocking in tests
#[cfg_attr(test, mockall::automock)]
pub trait EmailClient {
    /// Archives an email (removes from inbox)
    /// Returns the destination UID if COPYUID is supported by the server
    fn archive_email(&mut self, email_id: &str, folder: &str) -> Result<Option<u32>>;

    /// Deletes an email (moves to trash)
    /// Returns the destination UID if COPYUID is supported by the server
    fn delete_email(&mut self, email_id: &str, folder: &str) -> Result<Option<u32>>;

    /// Archives a batch of emails from a single folder (moves to All Mail)
    /// UIDs should be from the same folder for efficiency
    /// Returns a mapping of source UID -> destination UID (empty if COPYUID not supported)
    fn archive_batch(&mut self, uids: &[String], folder: &str) -> Result<HashMap<String, u32>>;

    /// Deletes a batch of emails from a single folder (moves to Trash)
    /// UIDs should be from the same folder for efficiency
    /// Returns a mapping of source UID -> destination UID (empty if COPYUID not supported)
    fn delete_batch(&mut self, uids: &[String], folder: &str) -> Result<HashMap<String, u32>>;

    /// Restores emails to their original folders
    /// Takes a list of (message_id, dest_uid, current_folder, destination_folder) tuples
    /// Uses dest_uid for fast restore if available, falls back to Message-ID search otherwise
    fn restore_emails(
        &mut self,
        emails: &[(Option<String>, Option<u32>, String, String)],
    ) -> Result<()>;
}

/// IMAP client for Gmail access
pub struct ImapClient {
    session: Session<Box<dyn ImapConnection>>,
}

/// Parses a COPYUID response to extract the mapping from source UIDs to destination UIDs
///
/// The COPYUID response format from RFC 4315 is:
/// COPYUID <uidvalidity> <source-uid-set> <dest-uid-set>
///
/// Returns a HashMap mapping source UID (as String) to destination UID
fn parse_copyuid_response(response: &[u8]) -> HashMap<String, u32> {
    use imap_proto::parser::parse_response;
    use imap_proto::types::{Response, ResponseCode};

    let mut uid_map = HashMap::new();

    // Parse response line by line (IMAP responses may have multiple lines)
    let mut remaining = response;
    while !remaining.is_empty() {
        match parse_response(remaining) {
            Ok((rest, resp)) => {
                // Check for Done response with COPYUID code (tagged response like "a1 OK [COPYUID ...]")
                if let Response::Done {
                    code: Some(ResponseCode::CopyUid(_, source_uids, dest_uids)),
                    ..
                } = resp
                {
                    // Expand UID sets into individual UIDs
                    let sources: Vec<u32> = expand_uid_set(&source_uids);
                    let dests: Vec<u32> = expand_uid_set(&dest_uids);

                    // Build the mapping
                    for (src, dst) in sources.iter().zip(dests.iter()) {
                        uid_map.insert(src.to_string(), *dst);
                    }
                    break;
                }
                remaining = rest;
            }
            Err(_) => {
                // If parsing fails, try skipping to the next line
                if let Some(pos) = remaining.iter().position(|&b| b == b'\n') {
                    remaining = &remaining[pos + 1..];
                } else {
                    break;
                }
            }
        }
    }

    uid_map
}

/// Expands a UID set (which may contain ranges) into individual UIDs
fn expand_uid_set(uid_set: &[imap_proto::types::UidSetMember]) -> Vec<u32> {
    use imap_proto::types::UidSetMember;

    let mut uids = Vec::new();
    for member in uid_set {
        match member {
            UidSetMember::Uid(uid) => uids.push(*uid),
            UidSetMember::UidRange(range) => {
                for uid in range.clone() {
                    uids.push(uid);
                }
            }
        }
    }
    uids
}

/// Extracts contiguous ranges from sorted UIDs
/// Example: [1,2,3,5,7,8,9] -> [(1,3), (5,5), (7,9)]
fn extract_uid_ranges(uids: &[u32]) -> Vec<(u32, u32)> {
    if uids.is_empty() {
        return Vec::new();
    }

    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut ranges = Vec::new();
    let mut start = sorted[0];
    let mut end = sorted[0];

    for &uid in &sorted[1..] {
        if uid == end + 1 {
            // Extend current range
            end = uid;
        } else {
            // Save current range and start a new one
            ranges.push((start, end));
            start = uid;
            end = uid;
        }
    }

    // Don't forget the last range
    ranges.push((start, end));

    ranges
}

/// Sends a UID MOVE command and returns the COPYUID mapping if available
///
/// This uses the IMAP MOVE extension (RFC 6851) combined with UIDPLUS (RFC 4315)
/// to atomically move messages and get their new UIDs in the destination folder.
fn uid_move_with_copyuid(
    session: &mut Session<Box<dyn ImapConnection>>,
    uids: &str,
    dest_mailbox: &str,
) -> Result<HashMap<String, u32>> {
    // Escape the mailbox name if it contains special characters
    let escaped_mailbox = if dest_mailbox.contains(' ') || dest_mailbox.contains('"') {
        format!("\"{}\"", dest_mailbox.replace('"', "\\\""))
    } else {
        dest_mailbox.to_string()
    };

    let command = format!("UID MOVE {} {}", uids, escaped_mailbox);
    crate::debug_log!("uid_move_with_copyuid: {}", command);

    // Use run() to get the full response including the tagged OK line with COPYUID
    let (response, tagged_start) = session.run(&command).context("UID MOVE command failed")?;

    // Parse the tagged response portion (which contains COPYUID)
    let tagged_response = &response[tagged_start..];
    let uid_map = parse_copyuid_response(tagged_response);
    crate::debug_log!("uid_move_with_copyuid: got {} UID mappings", uid_map.len());

    Ok(uid_map)
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
    fn archive_email(&mut self, uid: &str, folder: &str) -> Result<Option<u32>> {
        self.session
            .select(folder)
            .context(format!("Failed to select {}", folder))?;

        // Move to All Mail (Gmail's archive) and get destination UID
        let uid_map = uid_move_with_copyuid(&mut self.session, uid, "[Gmail]/All Mail")
            .context("Failed to archive email")?;

        // Return the destination UID if available
        Ok(uid_map.get(uid).copied())
    }

    fn delete_email(&mut self, uid: &str, folder: &str) -> Result<Option<u32>> {
        self.session
            .select(folder)
            .context(format!("Failed to select {}", folder))?;

        // Move to Trash and get destination UID
        let uid_map = uid_move_with_copyuid(&mut self.session, uid, "[Gmail]/Trash")
            .context("Failed to delete email")?;

        // Return the destination UID if available
        Ok(uid_map.get(uid).copied())
    }

    fn archive_batch(&mut self, uids: &[String], folder: &str) -> Result<HashMap<String, u32>> {
        if uids.is_empty() {
            return Ok(HashMap::new());
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
        let uid_map = uid_move_with_copyuid(&mut self.session, &uid_sequence, "[Gmail]/All Mail")
            .context("Failed to archive emails")?;

        crate::debug_log!("archive_batch: done, got {} UID mappings", uid_map.len());
        Ok(uid_map)
    }

    fn delete_batch(&mut self, uids: &[String], folder: &str) -> Result<HashMap<String, u32>> {
        if uids.is_empty() {
            return Ok(HashMap::new());
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
        let uid_map = uid_move_with_copyuid(&mut self.session, &uid_sequence, "[Gmail]/Trash")
            .context("Failed to delete emails")?;

        crate::debug_log!("delete_batch: done, got {} UID mappings", uid_map.len());
        Ok(uid_map)
    }

    fn restore_emails(
        &mut self,
        emails: &[(Option<String>, Option<u32>, String, String)],
    ) -> Result<()> {
        // Partition into batch-eligible (have dest_uid) and fallback (need Message-ID search)
        let mut batch_eligible: Vec<(u32, String, String)> = Vec::new();
        let mut fallback: Vec<(String, String, String)> = Vec::new();

        for (message_id, dest_uid, current_folder, dest_folder) in emails {
            if let Some(uid) = dest_uid {
                batch_eligible.push((*uid, current_folder.clone(), dest_folder.clone()));
            } else if let Some(msg_id) = message_id {
                fallback.push((msg_id.clone(), current_folder.clone(), dest_folder.clone()));
            }
            // Skip if neither dest_uid nor message_id is available
        }

        // Process batch-eligible emails: group by (current_folder, dest_folder) route
        let mut routes: HashMap<(String, String), Vec<u32>> = HashMap::new();
        for (uid, current_folder, dest_folder) in batch_eligible {
            routes
                .entry((current_folder, dest_folder))
                .or_default()
                .push(uid);
        }

        // Execute one UID MOVE per contiguous range for each route
        for ((current_folder, dest_folder), uids) in routes {
            self.session
                .select(&current_folder)
                .context(format!("Failed to select {}", current_folder))?;

            let ranges = extract_uid_ranges(&uids);
            crate::debug_log!(
                "restore_emails: batch moving {} UIDs in {} ranges from {} to {}",
                uids.len(),
                ranges.len(),
                current_folder,
                dest_folder
            );

            for (start, end) in ranges {
                let uid_range = if start == end {
                    start.to_string()
                } else {
                    format!("{}:{}", start, end)
                };

                crate::debug_log!("restore_emails: UID MOVE {} to {}", uid_range, dest_folder);
                uid_move_with_copyuid(&mut self.session, &uid_range, &dest_folder).context(
                    format!("Failed to restore UIDs {} to {}", uid_range, dest_folder),
                )?;
            }
        }

        // Process fallback emails individually using Message-ID search
        let mut current_selected: Option<&str> = None;
        for (msg_id, current_folder, dest_folder) in &fallback {
            // Select the folder if needed
            if current_selected != Some(current_folder.as_str()) {
                self.session
                    .select(current_folder)
                    .context(format!("Failed to select {}", current_folder))?;
                current_selected = Some(current_folder.as_str());
            }

            crate::debug_log!("restore_emails: using Message-ID fallback for {}", msg_id);
            let search_query = format!("HEADER Message-ID {}", msg_id);
            let uids = self
                .session
                .uid_search(&search_query)
                .context(format!("Failed to search for Message-ID {}", msg_id))?;

            if let Some(uid) = uids.into_iter().next() {
                self.session
                    .uid_mv(uid.to_string(), dest_folder)
                    .context(format!(
                        "Failed to restore email {} to {}",
                        msg_id, dest_folder
                    ))?;
            }
            // If email not found, it may have been permanently deleted or already moved
            // Continue with other emails rather than failing entirely
        }

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
            .returning(|_, _| Ok(Some(100))); // Returns destination UID

        let result = mock.archive_email("email123", "INBOX");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(100));
    }

    #[test]
    fn test_mock_client_delete() {
        let mut mock = MockEmailClient::new();

        mock.expect_delete_email()
            .with(
                mockall::predicate::eq("email456"),
                mockall::predicate::eq("INBOX"),
            )
            .returning(|_, _| Ok(Some(200))); // Returns destination UID

        let result = mock.delete_email("email456", "INBOX");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(200));
    }

    #[test]
    fn test_mock_client_restore() {
        let mut mock = MockEmailClient::new();

        mock.expect_restore_emails().returning(|_| Ok(()));

        // New format: (message_id, dest_uid, current_folder, dest_folder)
        let restore_ops = vec![
            (
                Some("<123@example.com>".to_string()),
                Some(100),
                "[Gmail]/All Mail".to_string(),
                "INBOX".to_string(),
            ),
            (
                Some("<456@example.com>".to_string()),
                Some(200),
                "[Gmail]/Trash".to_string(),
                "INBOX".to_string(),
            ),
        ];
        let result = mock.restore_emails(&restore_ops);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_client_restore_fallback() {
        // Test that restore works with only Message-ID (no dest_uid)
        let mut mock = MockEmailClient::new();

        mock.expect_restore_emails().returning(|_| Ok(()));

        // No dest_uid, should fall back to Message-ID search
        let restore_ops = vec![(
            Some("<789@example.com>".to_string()),
            None,
            "[Gmail]/All Mail".to_string(),
            "INBOX".to_string(),
        )];
        let result = mock.restore_emails(&restore_ops);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_uid_ranges_empty() {
        let ranges = extract_uid_ranges(&[]);
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_extract_uid_ranges_single() {
        let ranges = extract_uid_ranges(&[42]);
        assert_eq!(ranges, vec![(42, 42)]);
    }

    #[test]
    fn test_extract_uid_ranges_consecutive() {
        let ranges = extract_uid_ranges(&[1, 2, 3, 4, 5]);
        assert_eq!(ranges, vec![(1, 5)]);
    }

    #[test]
    fn test_extract_uid_ranges_scattered() {
        let ranges = extract_uid_ranges(&[1, 3, 5, 7, 9]);
        assert_eq!(ranges, vec![(1, 1), (3, 3), (5, 5), (7, 7), (9, 9)]);
    }

    #[test]
    fn test_extract_uid_ranges_mixed() {
        // Example from plan: [1,2,3,5,7,8,9] -> [(1,3), (5,5), (7,9)]
        let ranges = extract_uid_ranges(&[1, 2, 3, 5, 7, 8, 9]);
        assert_eq!(ranges, vec![(1, 3), (5, 5), (7, 9)]);
    }

    #[test]
    fn test_extract_uid_ranges_unsorted() {
        // Input doesn't need to be sorted
        let ranges = extract_uid_ranges(&[5, 1, 3, 2, 4]);
        assert_eq!(ranges, vec![(1, 5)]);
    }

    #[test]
    fn test_extract_uid_ranges_with_duplicates() {
        // Duplicates should be handled
        let ranges = extract_uid_ranges(&[1, 1, 2, 2, 3, 5, 5]);
        assert_eq!(ranges, vec![(1, 3), (5, 5)]);
    }
}
