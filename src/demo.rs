use chrono::{Duration, Utc};

use crate::email::{Email, EmailBuilder, build_thread_ids};

/// Creates a set of realistic demo emails for testing and screenshots
pub fn create_demo_emails() -> Vec<Email> {
    let now = Utc::now();
    let yesterday = now - Duration::days(1);
    let two_days_ago = now - Duration::days(2);
    let last_week = now - Duration::days(7);

    let mut emails = vec![
        // GitHub notifications (3 emails)
        EmailBuilder::new()
            .id("demo_1")
            .from("GitHub <notifications@github.com>")
            .subject("[rust-lang/rust] Fix ICE in pattern matching (PR #12345)")
            .snippet("@bors merged this pull request. The changes look good and all CI checks passed...")
            .date(now - Duration::hours(2))
            .message_id("<gh-pr-12345@github.com>")
            .source_folder("INBOX")
            .build(),
        EmailBuilder::new()
            .id("demo_2")
            .from("GitHub <notifications@github.com>")
            .subject("[tokio-rs/tokio] New issue: Memory leak in async runtime")
            .snippet("A new issue has been opened by @contributor. Steps to reproduce: 1. Create a new runtime...")
            .date(now - Duration::hours(5))
            .message_id("<gh-issue-456@github.com>")
            .source_folder("INBOX")
            .build(),
        EmailBuilder::new()
            .id("demo_3")
            .from("GitHub <notifications@github.com>")
            .subject("Your mass migration jobs are now available")
            .snippet("We're excited to announce that mass migration jobs are now available for your organization...")
            .date(yesterday)
            .message_id("<gh-announce-789@github.com>")
            .source_folder("INBOX")
            .build(),

        // Linear updates (2 emails)
        EmailBuilder::new()
            .id("demo_4")
            .from("Linear <notify@linear.app>")
            .subject("ENG-1234: Implement user authentication")
            .snippet("Status changed to In Review. Alice assigned this issue to you for final review...")
            .date(now - Duration::hours(1))
            .message_id("<linear-1234@linear.app>")
            .source_folder("INBOX")
            .build(),
        EmailBuilder::new()
            .id("demo_5")
            .from("Linear <notify@linear.app>")
            .subject("Weekly project digest - Sprint 42")
            .snippet("12 issues completed, 3 in progress, 5 remaining. Team velocity is up 15% from last week...")
            .date(two_days_ago)
            .message_id("<linear-digest@linear.app>")
            .source_folder("INBOX")
            .build(),

        // Personal thread with Alice (conversation)
        EmailBuilder::new()
            .id("demo_6")
            .from("Alice Chen <alice@example.com>")
            .subject("Re: Coffee tomorrow?")
            .snippet("That works! How about the new place on Market St? I heard they have great espresso...")
            .date(now - Duration::hours(3))
            .message_id("<alice-reply-2@example.com>")
            .in_reply_to("<demo-sent-1@example.com>")
            .references(vec![
                "<alice-orig@example.com>".to_string(),
                "<demo-sent-1@example.com>".to_string(),
            ])
            .source_folder("INBOX")
            .build(),
        EmailBuilder::new()
            .id("demo_7")
            .from("Alice Chen <alice@example.com>")
            .subject("Coffee tomorrow?")
            .snippet("Hey! It's been a while. Want to grab coffee tomorrow afternoon? I'm free after 2pm...")
            .date(yesterday - Duration::hours(2))
            .message_id("<alice-orig@example.com>")
            .source_folder("INBOX")
            .build(),
        // User's sent reply (will be in thread view)
        EmailBuilder::new()
            .id("demo_sent_1")
            .from("Demo User <demo@example.com>")
            .subject("Re: Coffee tomorrow?")
            .snippet("Sure! 3pm works for me. Any preference on location?")
            .date(yesterday)
            .message_id("<demo-sent-1@example.com>")
            .in_reply_to("<alice-orig@example.com>")
            .references(vec!["<alice-orig@example.com>".to_string()])
            .source_folder("[Gmail]/Sent Mail")
            .build(),

        // Stripe receipts (2 emails)
        EmailBuilder::new()
            .id("demo_8")
            .from("Stripe <receipts@stripe.com>")
            .subject("Your receipt from Acme Corp")
            .snippet("Amount: $49.00. Thank you for your payment. Your subscription has been renewed...")
            .date(two_days_ago)
            .message_id("<stripe-receipt-1@stripe.com>")
            .source_folder("INBOX")
            .build(),
        EmailBuilder::new()
            .id("demo_9")
            .from("Stripe <receipts@stripe.com>")
            .subject("Your receipt from Cloud Services Inc")
            .snippet("Amount: $12.00. Payment successful. Your monthly invoice is attached...")
            .date(last_week)
            .message_id("<stripe-receipt-2@stripe.com>")
            .source_folder("INBOX")
            .build(),

        // Figma updates
        EmailBuilder::new()
            .id("demo_10")
            .from("Figma <no-reply@figma.com>")
            .subject("Bob commented on 'Homepage Redesign'")
            .snippet("Bob: 'Love the new hero section! Can we try a darker shade for the CTA button?'...")
            .date(now - Duration::hours(4))
            .message_id("<figma-comment@figma.com>")
            .source_folder("INBOX")
            .build(),

        // Newsletter
        EmailBuilder::new()
            .id("demo_11")
            .from("This Week in Rust <noreply@this-week-in-rust.org>")
            .subject("This Week in Rust 542")
            .snippet("Hello and welcome to another issue of This Week in Rust! Updates from the community...")
            .date(two_days_ago + Duration::hours(4))
            .message_id("<twir-542@this-week-in-rust.org>")
            .source_folder("INBOX")
            .build(),

        // AWS notification
        EmailBuilder::new()
            .id("demo_12")
            .from("Amazon Web Services <no-reply@aws.amazon.com>")
            .subject("AWS Billing Alert: Your costs exceeded the threshold")
            .snippet("Your AWS account has exceeded the billing threshold of $100.00. Current charges: $127.43...")
            .date(yesterday + Duration::hours(6))
            .message_id("<aws-billing@aws.amazon.com>")
            .source_folder("INBOX")
            .build(),

        // Slack digest
        EmailBuilder::new()
            .id("demo_13")
            .from("Slack <feedback@slack.com>")
            .subject("Your daily digest from Acme Workspace")
            .snippet("You have 23 unread messages in 5 channels. #engineering: 12 new, #random: 5 new...")
            .date(now - Duration::hours(8))
            .message_id("<slack-digest@slack.com>")
            .source_folder("INBOX")
            .build(),

        // Another personal thread with Bob (multi-participant)
        EmailBuilder::new()
            .id("demo_14")
            .from("Bob Smith <bob@company.com>")
            .subject("Re: Q4 Planning")
            .snippet("I agree with Charlie's points. We should focus on the API improvements first...")
            .date(now - Duration::hours(6))
            .message_id("<bob-q4-reply@company.com>")
            .in_reply_to("<charlie-q4@company.com>")
            .references(vec![
                "<demo-q4-orig@example.com>".to_string(),
                "<charlie-q4@company.com>".to_string(),
            ])
            .source_folder("INBOX")
            .build(),
        EmailBuilder::new()
            .id("demo_15")
            .from("Charlie Davis <charlie@company.com>")
            .subject("Re: Q4 Planning")
            .snippet("Great overview! I think we should prioritize items 2 and 3 for the first milestone...")
            .date(yesterday + Duration::hours(3))
            .message_id("<charlie-q4@company.com>")
            .in_reply_to("<demo-q4-orig@example.com>")
            .references(vec!["<demo-q4-orig@example.com>".to_string()])
            .source_folder("INBOX")
            .build(),
        // User's original email starting the thread
        EmailBuilder::new()
            .id("demo_sent_2")
            .from("Demo User <demo@example.com>")
            .subject("Q4 Planning")
            .snippet("Hi team, I wanted to share my thoughts on Q4 priorities...")
            .date(yesterday)
            .message_id("<demo-q4-orig@example.com>")
            .source_folder("[Gmail]/Sent Mail")
            .build(),
    ];

    // Build thread IDs for the emails
    build_thread_ids(&mut emails);

    emails
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_demo_emails_created() {
        let emails = create_demo_emails();
        assert!(!emails.is_empty());
        // Should have at least 15 emails
        assert!(emails.len() >= 15);
    }

    #[test]
    fn test_demo_emails_have_unique_ids() {
        let emails = create_demo_emails();
        let ids: HashSet<_> = emails.iter().map(|e| &e.id).collect();
        assert_eq!(ids.len(), emails.len());
    }

    #[test]
    fn test_demo_emails_have_threads() {
        let emails = create_demo_emails();
        // All emails should have thread IDs
        assert!(emails.iter().all(|e| !e.thread_id.is_empty()));

        // Some threads should have multiple messages (the conversations)
        let thread_ids: HashSet<_> = emails.iter().map(|e| &e.thread_id).collect();
        let multi_message_threads = thread_ids
            .iter()
            .filter(|tid| emails.iter().filter(|e| &e.thread_id == **tid).count() > 1)
            .count();
        assert!(
            multi_message_threads >= 2,
            "Should have at least 2 multi-message threads"
        );
    }

    #[test]
    fn test_demo_emails_have_sent_mail() {
        let emails = create_demo_emails();
        let sent_count = emails
            .iter()
            .filter(|e| e.source_folder == "[Gmail]/Sent Mail")
            .count();
        assert!(sent_count >= 2, "Should have at least 2 sent emails");
    }

    #[test]
    fn test_demo_emails_have_variety_of_senders() {
        let emails = create_demo_emails();
        let domains: HashSet<_> = emails.iter().map(|e| &e.from_domain).collect();
        assert!(
            domains.len() >= 5,
            "Should have at least 5 different domains"
        );
    }
}
