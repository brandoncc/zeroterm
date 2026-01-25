use chrono::{DateTime, Utc};
use regex::Regex;

/// Represents an email message from Gmail
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
}

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
        }
    }
}

/// Extracts the email address from a "Name <email>" format string
/// If no angle brackets are present, returns the string trimmed as-is
pub fn extract_email(from: &str) -> String {
    let re = Regex::new(r"<([^>]+)>").unwrap();
    if let Some(captures) = re.captures(from) {
        captures.get(1).map(|m| m.as_str().to_string()).unwrap_or_default()
    } else {
        from.trim().to_string()
    }
}

/// Extracts the domain from an email address.
/// If no `@` is present, returns the full email to avoid grouping unrelated
/// malformed addresses together.
pub fn extract_domain(email: &str) -> String {
    email
        .split('@')
        .nth(1)
        .unwrap_or(email)
        .to_string()
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
}
