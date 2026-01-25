use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use google_gmail1::api::{Message, ModifyMessageRequest};
use google_gmail1::Gmail;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use yup_oauth2::authenticator::Authenticator;

use crate::email::Email;

/// Trait for Gmail operations - allows mocking in tests
#[cfg_attr(test, mockall::automock)]
pub trait GmailClient: Send + Sync {
    /// Fetches inbox emails
    fn fetch_inbox(&self) -> impl std::future::Future<Output = Result<Vec<Email>>> + Send;

    /// Archives an email (removes from inbox)
    fn archive_email(&self, email_id: &str) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Deletes an email (moves to trash)
    fn delete_email(&self, email_id: &str) -> impl std::future::Future<Output = Result<()>> + Send;
}

/// Real Gmail API client
pub struct RealGmailClient {
    hub: Gmail<HttpsConnector<HttpConnector>>,
}

impl RealGmailClient {
    /// Creates a new Gmail client with the given authenticator
    pub async fn new(
        auth: Authenticator<HttpsConnector<HttpConnector>>,
    ) -> Result<Self> {
        let client = google_gmail1::hyper_util::client::legacy::Client::builder(
            google_gmail1::hyper_util::rt::TokioExecutor::new(),
        )
        .build(
            google_gmail1::hyper_rustls::HttpsConnectorBuilder::new()
                .with_native_roots()
                .context("Failed to load native TLS roots")?
                .https_or_http()
                .enable_http1()
                .build(),
        );

        let hub = Gmail::new(client, auth);
        Ok(Self { hub })
    }

    /// Parses a Gmail message into our Email struct
    fn parse_message(&self, msg: &Message) -> Option<Email> {
        let id = msg.id.clone()?;
        let thread_id = msg.thread_id.clone().unwrap_or_default();
        let snippet = msg.snippet.clone().unwrap_or_default();

        let payload = msg.payload.as_ref()?;
        let headers = payload.headers.as_ref()?;

        let mut from = String::new();
        let mut subject = String::new();
        let mut date_str = String::new();

        for header in headers {
            if let (Some(name), Some(value)) = (&header.name, &header.value) {
                match name.to_lowercase().as_str() {
                    "from" => from = value.clone(),
                    "subject" => subject = value.clone(),
                    "date" => date_str = value.clone(),
                    _ => {}
                }
            }
        }

        let date = parse_email_date(&date_str).unwrap_or_else(Utc::now);

        Some(Email::new(id, thread_id, from, subject, snippet, date))
    }
}

impl GmailClient for RealGmailClient {
    async fn fetch_inbox(&self) -> Result<Vec<Email>> {
        let mut emails = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut request = self
                .hub
                .users()
                .messages_list("me")
                .q("in:inbox")
                .max_results(100);

            if let Some(token) = &page_token {
                request = request.page_token(token);
            }

            let (_, response) = request.doit().await.context("Failed to list messages")?;

            if let Some(messages) = response.messages {
                for msg_ref in messages {
                    if let Some(msg_id) = msg_ref.id {
                        // Fetch full message details
                        let (_, msg) = self
                            .hub
                            .users()
                            .messages_get("me", &msg_id)
                            .format("metadata")
                            .add_metadata_headers("From")
                            .add_metadata_headers("Subject")
                            .add_metadata_headers("Date")
                            .doit()
                            .await
                            .context("Failed to get message")?;

                        if let Some(email) = self.parse_message(&msg) {
                            emails.push(email);
                        }
                    }
                }
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        Ok(emails)
    }

    async fn archive_email(&self, email_id: &str) -> Result<()> {
        let req = ModifyMessageRequest {
            remove_label_ids: Some(vec!["INBOX".to_string()]),
            ..Default::default()
        };

        self.hub
            .users()
            .messages_modify(req, "me", email_id)
            .doit()
            .await
            .context("Failed to archive email")?;

        Ok(())
    }

    async fn delete_email(&self, email_id: &str) -> Result<()> {
        self.hub
            .users()
            .messages_trash("me", email_id)
            .doit()
            .await
            .context("Failed to delete email")?;

        Ok(())
    }
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

    // Mock client tests
    #[tokio::test]
    async fn test_mock_client_fetch() {
        use chrono::Utc;

        let mut mock = MockGmailClient::new();

        mock.expect_fetch_inbox().returning(|| {
            Box::pin(async {
                Ok(vec![Email::new(
                    "1".to_string(),
                    "t1".to_string(),
                    "test@example.com".to_string(),
                    "Subject".to_string(),
                    "Snippet".to_string(),
                    Utc::now(),
                )])
            })
        });

        let emails = mock.fetch_inbox().await.unwrap();
        assert_eq!(emails.len(), 1);
    }

    #[tokio::test]
    async fn test_mock_client_archive() {
        let mut mock = MockGmailClient::new();

        mock.expect_archive_email()
            .with(mockall::predicate::eq("email123"))
            .returning(|_| Box::pin(async { Ok(()) }));

        let result = mock.archive_email("email123").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_client_delete() {
        let mut mock = MockGmailClient::new();

        mock.expect_delete_email()
            .with(mockall::predicate::eq("email456"))
            .returning(|_| Box::pin(async { Ok(()) }));

        let result = mock.delete_email("email456").await;
        assert!(result.is_ok());
    }
}

