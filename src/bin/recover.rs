//! Recovery tool to check and restore recently deleted emails from Gmail Trash
//!
//! Usage:
//!   cargo run --bin recover -- --check
//!   cargo run --bin recover -- --check --from "someone@example.com"
//!   cargo run --bin recover -- --restore --from "notifications@github.com"

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::env;

fn parse_args() -> Result<Option<Args>> {
    let args: Vec<String> = env::args().collect();

    let check_mode = args.iter().any(|a| a == "--check");
    let restore_mode = args.iter().any(|a| a == "--restore");
    let list_accounts = args.iter().any(|a| a == "--list-accounts");

    let account_name = args
        .iter()
        .position(|a| a == "--account")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let from_filter = args
        .iter()
        .position(|a| a == "--from")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.to_lowercase());

    let to_folder = args
        .iter()
        .position(|a| a == "--to")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "INBOX".to_string());

    let limit: usize = args
        .iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);

    let count: Option<usize> = args
        .iter()
        .position(|a| a == "--count")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    if !check_mode && !restore_mode && !list_accounts {
        print_usage();
        return Ok(None);
    }

    Ok(Some(Args {
        check_mode,
        restore_mode,
        list_accounts,
        account_name,
        from_filter,
        to_folder,
        limit,
        count,
    }))
}

struct Args {
    check_mode: bool,
    restore_mode: bool,
    list_accounts: bool,
    account_name: Option<String>,
    from_filter: Option<String>,
    to_folder: String,
    limit: usize,
    count: Option<usize>,
}

fn print_usage() {
    println!("Email Recovery Tool");
    println!("===================");
    println!();
    println!("Recover accidentally deleted emails from Gmail Trash.");
    println!();
    println!("USAGE:");
    println!("  cargo run --bin recover -- [OPTIONS] <COMMAND>");
    println!();
    println!("COMMANDS:");
    println!("  --check           List recent emails in Trash with sender counts");
    println!("  --restore         Move matching emails back to destination folder");
    println!("  --list-accounts   List available accounts from config");
    println!();
    println!("OPTIONS:");
    println!("  --account <NAME>  Account name from config (required)");
    println!("  --from <SENDER>   Filter by sender email (partial match, case-insensitive)");
    println!("  --to <FOLDER>     Destination folder for restore (default: INBOX)");
    println!("  --limit <N>       How many recent emails to fetch (default: 2000)");
    println!("  --count <N>       Only process/restore this many emails (most recent first)");
    println!();
    println!("EXAMPLES:");
    println!("  # List accounts");
    println!("  cargo run --bin recover -- --list-accounts");
    println!();
    println!("  # Check recently deleted emails (shows top senders)");
    println!("  cargo run --bin recover -- --account personal --check");
    println!();
    println!("  # Check deleted emails from a specific sender");
    println!(
        "  cargo run --bin recover -- --account personal --check --from \"notifications@github.com\""
    );
    println!();
    println!("  # Restore the last 1841 deleted emails (no sender filter)");
    println!("  cargo run --bin recover -- --account personal --restore --count 1841");
    println!();
    println!("  # Restore deleted GitHub notifications to INBOX");
    println!(
        "  cargo run --bin recover -- --account personal --restore --from \"notifications@github.com\""
    );
    println!();
}

fn main() -> Result<()> {
    let args = match parse_args()? {
        Some(a) => a,
        None => return Ok(()),
    };

    // Load config
    let config = load_config()?;

    if args.list_accounts {
        println!("Available accounts:");
        for (name, account) in &config {
            println!("  {} ({})", name, account.email);
        }
        return Ok(());
    }

    // Select account (required)
    let account_name = args
        .account_name
        .as_ref()
        .context("--account <NAME> is required. Use --list-accounts to see available accounts.")?;
    let account = config
        .get(account_name)
        .with_context(|| format!("Account '{}' not found in config", account_name))?;

    println!("Connecting to {} ({})...", account_name, account.email);

    let client = imap::ClientBuilder::new("imap.gmail.com", 993)
        .connect()
        .context("Failed to connect to IMAP server")?;

    let mut session = client
        .login(&account.email, &account.password)
        .map_err(|e| anyhow::anyhow!("Login failed: {}", e.0))?;

    // Select Trash folder
    let mailbox = session
        .select("[Gmail]/Trash")
        .context("Failed to select [Gmail]/Trash")?;

    let total = mailbox.exists;
    println!("[Gmail]/Trash contains {} emails", total);

    if total == 0 {
        println!("Folder is empty!");
        return Ok(());
    }

    // Fetch the most recent emails (highest sequence numbers = most recent)
    let start = if total > args.limit as u32 {
        total - args.limit as u32 + 1
    } else {
        1
    };
    let range = format!("{}:{}", start, total);

    println!(
        "Fetching emails {} to {} ({} emails)...",
        start,
        total,
        total - start + 1
    );

    let messages = session
        .fetch(&range, "(UID ENVELOPE)")
        .context("Failed to fetch messages")?;

    #[derive(Debug)]
    struct RecoverEmail {
        uid: u32,
        from: String,
        subject: String,
        date: Option<DateTime<Utc>>,
    }

    let mut emails: Vec<RecoverEmail> = Vec::new();

    for msg in messages.iter() {
        let uid = match msg.uid {
            Some(u) => u,
            None => continue,
        };

        let envelope = match msg.envelope() {
            Some(e) => e,
            None => continue,
        };

        let from = envelope
            .from
            .as_ref()
            .and_then(|addrs| addrs.first())
            .map(|addr| {
                let mailbox = addr
                    .mailbox
                    .as_ref()
                    .map(|m| String::from_utf8_lossy(m).to_string())
                    .unwrap_or_default();
                let host = addr
                    .host
                    .as_ref()
                    .map(|h| String::from_utf8_lossy(h).to_string())
                    .unwrap_or_default();
                format!("{}@{}", mailbox, host)
            })
            .unwrap_or_else(|| "unknown".to_string());

        let subject = envelope
            .subject
            .as_ref()
            .map(|s| String::from_utf8_lossy(s).to_string())
            .unwrap_or_default();

        let date = envelope.date.as_ref().and_then(|d| {
            let date_str = String::from_utf8_lossy(d);
            DateTime::parse_from_rfc2822(&date_str)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        });

        emails.push(RecoverEmail {
            uid,
            from,
            subject,
            date,
        });
    }

    // Sort by UID descending - higher UID = more recently added to folder
    // For Trash, this means most recently deleted first
    emails.sort_by(|a, b| b.uid.cmp(&a.uid));

    println!("Parsed {} emails from [Gmail]/Trash", emails.len());
    println!();

    if args.check_mode {
        if let Some(ref filter) = args.from_filter {
            // Filter by sender
            let matching: Vec<_> = emails
                .iter()
                .filter(|e| e.from.to_lowercase().contains(filter))
                .collect();

            println!(
                "Found {} deleted emails from senders matching '{}':",
                matching.len(),
                filter
            );
            println!();

            for email in matching.iter().take(50) {
                let date_str = email
                    .date
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "unknown date".to_string());
                let subject_preview: String = email.subject.chars().take(60).collect();
                println!("  [{}] {} - {}", date_str, email.from, subject_preview);
            }

            if matching.len() > 50 {
                println!("  ... and {} more", matching.len() - 50);
            }
        } else {
            // Show sender summary
            let mut sender_counts: HashMap<String, usize> = HashMap::new();
            for email in &emails {
                *sender_counts.entry(email.from.clone()).or_insert(0) += 1;
            }

            let mut counts: Vec<_> = sender_counts.into_iter().collect();
            counts.sort_by(|a, b| b.1.cmp(&a.1));

            println!(
                "Top senders in [Gmail]/Trash (most recent {} emails):",
                emails.len()
            );
            println!();
            for (sender, count) in counts.iter().take(30) {
                println!("  {:>5}  {}", count, sender);
            }

            if counts.len() > 30 {
                println!("  ... and {} more senders", counts.len() - 30);
            }
        }
    }

    if args.restore_mode {
        // Build list of matching UIDs
        let mut matching_uids: Vec<u32> = if let Some(ref filter) = args.from_filter {
            emails
                .iter()
                .filter(|e| e.from.to_lowercase().contains(filter.as_str()))
                .map(|e| e.uid)
                .collect()
        } else {
            // No filter - use all emails (requires --count for safety)
            if args.count.is_none() {
                anyhow::bail!("--restore without --from requires --count to prevent accidents");
            }
            emails.iter().map(|e| e.uid).collect()
        };

        // Apply count limit (take from beginning since emails are sorted by UID desc)
        if let Some(count) = args.count {
            matching_uids.truncate(count);
        }

        if matching_uids.is_empty() {
            if let Some(ref filter) = args.from_filter {
                println!("No deleted emails found matching '{}'", filter);
            } else {
                println!("No deleted emails found");
            }
            return Ok(());
        }

        // Build description for confirmation
        let filter_desc = if let Some(ref filter) = args.from_filter {
            format!("from '{}'", filter)
        } else {
            "(all senders)".to_string()
        };

        println!(
            "Found {} deleted emails to restore {}",
            matching_uids.len(),
            filter_desc
        );
        println!();
        print!(
            "Are you sure you want to move these to {}? [y/N] ",
            args.to_folder
        );

        use std::io::{self, Write};
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if input.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }

        println!("Restoring emails to {}...", args.to_folder);

        // Move in batches
        const BATCH_SIZE: usize = 100;
        let mut restored = 0;

        for chunk in matching_uids.chunks(BATCH_SIZE) {
            let uid_list: String = chunk
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(",");

            session
                .uid_mv(&uid_list, &args.to_folder)
                .context("Failed to move emails")?;

            restored += chunk.len();
            println!("  Restored {} / {} emails", restored, matching_uids.len());
        }

        println!();
        println!("Done! Restored {} emails to {}", restored, args.to_folder);
    }

    session.logout().ok();
    Ok(())
}

struct Account {
    email: String,
    password: String,
}

fn load_config() -> Result<HashMap<String, Account>> {
    // Try to load from zeroterm config
    let xdg_dirs = xdg::BaseDirectories::with_prefix("zeroterm")
        .context("Failed to determine config directory")?;
    let config_path = xdg_dirs.get_config_home().join("config.toml");

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config from {:?}", config_path))?;

    let config: toml::Value = toml::from_str(&content).context("Failed to parse config.toml")?;

    let accounts_table = config
        .get("accounts")
        .and_then(|a| a.as_table())
        .context("No accounts in config")?;

    let mut accounts = HashMap::new();

    for (name, account_value) in accounts_table {
        let email = account_value
            .get("email")
            .and_then(|e| e.as_str())
            .context("Missing email")?
            .to_string();

        let password_raw = account_value
            .get("app_password")
            .and_then(|p| p.as_str())
            .context("Missing app_password")?;

        // Resolve 1Password references
        let password = if password_raw.starts_with("op://") {
            let output = std::process::Command::new("op")
                .args(["read", password_raw])
                .output()
                .context("Failed to run 'op' command")?;

            if !output.status.success() {
                anyhow::bail!("Failed to read from 1Password");
            }

            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            password_raw.to_string()
        };

        accounts.insert(name.clone(), Account { email, password });
    }

    Ok(accounts)
}
