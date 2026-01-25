use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use imap::{ImapConnection, Session};

use crate::email::{build_thread_ids, Email};

/// Trait for email operations - allows mocking in tests
#[cfg_attr(test, mockall::automock)]
pub trait EmailClient {
    /// Fetches inbox emails
    fn fetch_inbox(&mut self) -> Result<Vec<Email>>;

    /// Archives an email (removes from inbox)
    fn archive_email(&mut self, email_id: &str) -> Result<()>;

    /// Deletes an email (moves to trash)
    fn delete_email(&mut self, email_id: &str) -> Result<()>;
}

/// IMAP client for Gmail access
pub struct ImapClient {
    session: Session<Box<dyn ImapConnection>>,
}

impl ImapClient {
    /// Creates a new IMAP client and connects to Gmail
    pub fn connect(email: &str, password: &str) -> Result<Self> {
        let client = imap::ClientBuilder::new("imap.gmail.com", 993)
            .connect()
            .context("Failed to connect to IMAP server")?;

        let session = client
            .login(email, password)
            .map_err(|e| anyhow::anyhow!("Login failed: {}", e.0))?;

        Ok(Self { session })
    }

    /// Parses an IMAP message into our Email struct
    fn parse_message(&self, fetch: &imap::types::Fetch) -> Option<Email> {
        let uid = fetch.uid?;
        let envelope = fetch.envelope()?;

        // Extract From
        let from = envelope.from.as_ref().and_then(|addrs| {
            addrs.first().map(|addr| {
                let name = addr.name.as_ref().map(|n| {
                    String::from_utf8_lossy(n).to_string()
                });
                let mailbox = addr.mailbox.as_ref().map(|m| {
                    String::from_utf8_lossy(m).to_string()
                });
                let host = addr.host.as_ref().map(|h| {
                    String::from_utf8_lossy(h).to_string()
                });

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

        // Parse headers for Message-ID and In-Reply-To
        let (message_id, in_reply_to) = fetch
            .header()
            .map(|h| parse_threading_headers(h))
            .unwrap_or((None, None));

        // Create snippet from first part of subject for now
        // (full body parsing would require fetching BODY[TEXT])
        let snippet = subject.chars().take(100).collect();

        Some(Email::with_headers(
            uid.to_string(),
            from,
            subject,
            snippet,
            date,
            message_id,
            in_reply_to,
        ))
    }

    /// Logs out and closes the connection
    #[allow(dead_code)]
    pub fn logout(mut self) -> Result<()> {
        self.session.logout().context("Failed to logout")?;
        Ok(())
    }
}

impl EmailClient for ImapClient {
    fn fetch_inbox(&mut self) -> Result<Vec<Email>> {
        self.session
            .select("INBOX")
            .context("Failed to select INBOX")?;

        // Check if there are any messages
        let mailbox = self.session.select("INBOX")?;
        if mailbox.exists == 0 {
            return Ok(Vec::new());
        }

        // Fetch all messages with headers
        let messages = self
            .session
            .fetch("1:*", "(UID ENVELOPE BODY.PEEK[HEADER])")
            .context("Failed to fetch messages")?;

        let mut emails = Vec::new();
        for msg in messages.iter() {
            if let Some(email) = self.parse_message(&msg) {
                emails.push(email);
            }
        }

        // Build thread IDs from Message-ID/In-Reply-To headers
        build_thread_ids(&mut emails);

        Ok(emails)
    }

    fn archive_email(&mut self, uid: &str) -> Result<()> {
        self.session
            .select("INBOX")
            .context("Failed to select INBOX")?;

        // Move to All Mail (Gmail's archive)
        self.session
            .uid_mv(uid, "[Gmail]/All Mail")
            .context("Failed to archive email")?;

        Ok(())
    }

    fn delete_email(&mut self, uid: &str) -> Result<()> {
        self.session
            .select("INBOX")
            .context("Failed to select INBOX")?;

        // Move to Trash
        self.session
            .uid_mv(uid, "[Gmail]/Trash")
            .context("Failed to delete email")?;

        Ok(())
    }
}

/// Decodes a potentially MIME-encoded header value
fn decode_header_value(value: &[u8]) -> String {
    // Try to parse as MIME encoded-word
    let value_str = String::from_utf8_lossy(value);

    // Handle =?charset?encoding?text?= format
    if value_str.contains("=?") {
        if let Ok(decoded) = mailparse::parse_header(format!("X: {}", value_str).as_bytes()) {
            return decoded.0.get_value();
        }
    }

    value_str.to_string()
}

/// Parses Message-ID and In-Reply-To headers from raw header bytes
fn parse_threading_headers(headers: &[u8]) -> (Option<String>, Option<String>) {
    let headers_str = String::from_utf8_lossy(headers);
    let mut message_id = None;
    let mut in_reply_to = None;

    for line in headers_str.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.starts_with("message-id:") {
            message_id = Some(line[11..].trim().to_string());
        } else if line_lower.starts_with("in-reply-to:") {
            in_reply_to = Some(line[12..].trim().to_string());
        }
    }

    (message_id, in_reply_to)
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
        let (msg_id, reply_to) = parse_threading_headers(headers);
        assert_eq!(msg_id, Some("<abc123@example.com>".to_string()));
        assert_eq!(reply_to, Some("<def456@example.com>".to_string()));
    }

    #[test]
    fn test_parse_threading_headers_no_reply() {
        let headers = b"Message-ID: <abc123@example.com>\r\nSubject: Test\r\n";
        let (msg_id, reply_to) = parse_threading_headers(headers);
        assert_eq!(msg_id, Some("<abc123@example.com>".to_string()));
        assert_eq!(reply_to, None);
    }

    // Mock client tests
    #[test]
    fn test_mock_client_fetch() {
        use chrono::Utc;

        let mut mock = MockEmailClient::new();

        mock.expect_fetch_inbox().returning(|| {
            Ok(vec![Email::new(
                "1".to_string(),
                "t1".to_string(),
                "test@example.com".to_string(),
                "Subject".to_string(),
                "Snippet".to_string(),
                Utc::now(),
            )])
        });

        let emails = mock.fetch_inbox().unwrap();
        assert_eq!(emails.len(), 1);
    }

    #[test]
    fn test_mock_client_archive() {
        let mut mock = MockEmailClient::new();

        mock.expect_archive_email()
            .with(mockall::predicate::eq("email123"))
            .returning(|_| Ok(()));

        let result = mock.archive_email("email123");
        assert!(result.is_ok());
    }

    #[test]
    fn test_mock_client_delete() {
        let mut mock = MockEmailClient::new();

        mock.expect_delete_email()
            .with(mockall::predicate::eq("email456"))
            .returning(|_| Ok(()));

        let result = mock.delete_email("email456");
        assert!(result.is_ok());
    }
}
