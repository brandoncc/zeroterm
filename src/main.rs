mod app;
mod config;
mod email;
mod imap_client;
mod ui;

use std::io;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use app::{App, View};
use config::AccountConfig;
use email::Email;
use imap_client::{EmailClient, ImapClient};
use ui::render::{render, render_account_select};
use ui::widgets::{AccountSelection, ConfirmAction, UiState};

/// Commands sent to the IMAP worker thread
enum ImapCommand {
    FetchInbox,
    ArchiveEmail(String),
    DeleteEmail(String),
    ArchiveMultiple(Vec<String>),
    DeleteMultiple(Vec<String>),
    Shutdown,
}

/// Responses from the IMAP worker thread
enum ImapResponse {
    Emails(Result<Vec<Email>>),
    ArchiveResult(Result<()>),
    DeleteResult(Result<()>),
    MultiArchiveResult(Result<()>),
    MultiDeleteResult(Result<()>),
    Connected,
    Error(String),
}

fn main() -> Result<()> {
    // Initialize
    config::ensure_config_dir()?;

    // Check for config
    if !config::has_config() {
        eprintln!(
            "Error: Configuration not found.\n\
             Please create a config.toml file at {:?}\n\
             with the following format:\n\n\
             [accounts.personal]\n\
             backend = \"gmail\"\n\
             email = \"your.email@gmail.com\"\n\
             app_password = \"xxxx xxxx xxxx xxxx\"\n\n\
             Create an App Password at: https://myaccount.google.com/apppasswords",
            config::config_path()?
        );
        std::process::exit(1);
    }

    // Load config
    let cfg = config::load_config()?;

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Select account (if multiple) or use the only one
    let selected_account = if cfg.accounts.len() > 1 {
        select_account(&mut terminal, &cfg)?
    } else {
        let (name, account) = config::get_default_account(&cfg)?;
        Some((name.clone(), account.clone()))
    };

    // User may have quit during account selection
    let result = if let Some(account) = selected_account {
        run_app(&mut terminal, account)
    } else {
        Ok(())
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    Ok(())
}

/// Shows account selection UI and returns the selected account
fn select_account(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    cfg: &config::Config,
) -> Result<Option<(String, AccountConfig)>> {
    let mut accounts: Vec<(String, AccountConfig)> = cfg
        .accounts
        .iter()
        .map(|(name, account)| (name.clone(), account.clone()))
        .collect();

    // Sort accounts alphabetically by name (case-insensitive)
    accounts.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut selection = AccountSelection::new(accounts);

    loop {
        terminal.draw(|f| render_account_select(f, &selection))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Char('q') => {
                    return Ok(None);
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    selection.select_next();
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    selection.select_previous();
                }
                KeyCode::Enter => {
                    if let Some((name, account)) = selection.current_account() {
                        return Ok(Some((name.clone(), account.clone())));
                    }
                }
                _ => {}
            }
        }
    }
}

/// Spawns the IMAP worker thread
fn spawn_imap_worker(
    cmd_rx: mpsc::Receiver<ImapCommand>,
    resp_tx: mpsc::Sender<ImapResponse>,
    account: AccountConfig,
) {
    thread::spawn(move || {
        let mut client = match ImapClient::connect(&account.email, &account.app_password) {
            Ok(c) => c,
            Err(e) => {
                let _ = resp_tx.send(ImapResponse::Error(format!("Failed to connect: {}", e)));
                return;
            }
        };

        let _ = resp_tx.send(ImapResponse::Connected);

        // Process commands
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                ImapCommand::FetchInbox => {
                    let result = client.fetch_inbox();
                    let _ = resp_tx.send(ImapResponse::Emails(result));
                }
                ImapCommand::ArchiveEmail(id) => {
                    let result = client.archive_email(&id);
                    let _ = resp_tx.send(ImapResponse::ArchiveResult(result));
                }
                ImapCommand::DeleteEmail(id) => {
                    let result = client.delete_email(&id);
                    let _ = resp_tx.send(ImapResponse::DeleteResult(result));
                }
                ImapCommand::ArchiveMultiple(ids) => {
                    let mut result = Ok(());
                    for id in &ids {
                        if let Err(e) = client.archive_email(id) {
                            result = Err(e);
                            break;
                        }
                    }
                    let _ = resp_tx.send(ImapResponse::MultiArchiveResult(result));
                }
                ImapCommand::DeleteMultiple(ids) => {
                    let mut result = Ok(());
                    for id in &ids {
                        if let Err(e) = client.delete_email(id) {
                            result = Err(e);
                            break;
                        }
                    }
                    let _ = resp_tx.send(ImapResponse::MultiDeleteResult(result));
                }
                ImapCommand::Shutdown => {
                    break;
                }
            }
        }

        // Properly close the IMAP session
        let _ = client.logout();
    });
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    account: (String, AccountConfig),
) -> Result<()> {
    let (account_name, account_config) = account;
    let mut app = App::new();
    let mut ui_state = UiState::new();

    // Create channels for IMAP communication
    let (cmd_tx, cmd_rx) = mpsc::channel::<ImapCommand>();
    let (resp_tx, resp_rx) = mpsc::channel::<ImapResponse>();

    // Show connecting status
    ui_state.set_busy(format!("Connecting to {}...", account_name));
    terminal.draw(|f| render(f, &app, &mut ui_state))?;

    // Spawn IMAP worker thread
    spawn_imap_worker(cmd_rx, resp_tx, account_config);

    // Wait for connection
    loop {
        // Check for responses
        match resp_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ImapResponse::Connected) => {
                ui_state.set_busy("Loading emails...");
                terminal.draw(|f| render(f, &app, &mut ui_state))?;
                cmd_tx.send(ImapCommand::FetchInbox)?;
                break;
            }
            Ok(ImapResponse::Error(e)) => {
                return Err(anyhow::anyhow!("{}", e));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Keep waiting, but check for quit
                if event::poll(Duration::from_millis(0))?
                    && let Event::Key(key) = event::read()?
                    && key.code == KeyCode::Char('q')
                {
                    let _ = cmd_tx.send(ImapCommand::Shutdown);
                    return Ok(());
                }
            }
            _ => {}
        }
    }

    // Track pending operations
    let mut pending_operation: Option<PendingOp> = None;
    // Track pending 'g' for gg sequence
    let mut pending_g = false;

    // Main event loop
    loop {
        // Tick spinner animation when busy
        if ui_state.is_busy() {
            ui_state.tick_spinner();
        }

        terminal.draw(|f| render(f, &app, &mut ui_state))?;

        // Check for IMAP responses (non-blocking)
        while let Ok(response) = resp_rx.try_recv() {
            match response {
                ImapResponse::Emails(result) => match result {
                    Ok(emails) => {
                        app.set_emails(emails);
                        ui_state.clear_busy();
                    }
                    Err(e) => {
                        ui_state.clear_busy();
                        ui_state.set_status(format!("Error: {}", e));
                    }
                },
                ImapResponse::ArchiveResult(result) => {
                    if let Some(PendingOp::ArchiveSingle(id)) = pending_operation.take()
                        && result.is_ok()
                    {
                        app.remove_email(&id);
                    }
                    ui_state.clear_busy();
                }
                ImapResponse::DeleteResult(result) => {
                    if let Some(PendingOp::DeleteSingle(id)) = pending_operation.take()
                        && result.is_ok()
                    {
                        app.remove_email(&id);
                    }
                    ui_state.clear_busy();
                }
                ImapResponse::MultiArchiveResult(result) => {
                    if let Some(op) = pending_operation.take()
                        && result.is_ok()
                    {
                        match op {
                            PendingOp::ArchiveGroup => {
                                app.remove_current_group_emails();
                            }
                            PendingOp::ArchiveThread(ref thread_id) => {
                                app.remove_thread(thread_id);
                                if app.view == View::Thread {
                                    app.exit();
                                }
                            }
                            _ => {}
                        }
                    }
                    ui_state.clear_busy();
                }
                ImapResponse::MultiDeleteResult(result) => {
                    if let Some(op) = pending_operation.take()
                        && result.is_ok()
                    {
                        match op {
                            PendingOp::DeleteGroup => {
                                app.remove_current_group_emails();
                            }
                            PendingOp::DeleteThread(ref thread_id) => {
                                app.remove_thread(thread_id);
                                if app.view == View::Thread {
                                    app.exit();
                                }
                            }
                            _ => {}
                        }
                    }
                    ui_state.clear_busy();
                }
                _ => {}
            }
        }

        // Poll for keyboard events with timeout
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Block input when busy (except we still consume events to avoid queue buildup)
            if ui_state.is_busy() {
                continue;
            }

            // Handle confirmation dialog input
            if ui_state.is_confirming() {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        if let Some(action) = ui_state.confirm_action.take() {
                            handle_confirmed_action(
                                &mut app,
                                &cmd_tx,
                                &mut ui_state,
                                &mut pending_operation,
                                action,
                            )?;
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        ui_state.clear_confirm();
                    }
                    _ => {}
                }
                continue;
            }

            // Clear pending g for any key that's not part of the gg sequence
            let is_g_sequence = matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G'));
            if !is_g_sequence {
                pending_g = false;
            }

            // Normal input handling
            match key.code {
                KeyCode::Char('q') => {
                    if app.view == View::GroupList {
                        let _ = cmd_tx.send(ImapCommand::Shutdown);
                        break;
                    } else {
                        app.exit();
                    }
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let half_page = ui_state.viewport_heights.for_view(app.view) / 2;
                    app.select_next_n(half_page.max(1));
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let half_page = ui_state.viewport_heights.for_view(app.view) / 2;
                    app.select_previous_n(half_page.max(1));
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    app.select_next();
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.select_previous();
                }
                KeyCode::Enter => {
                    if app.view == View::Thread {
                        // Open email in browser for security (avoids terminal escape attacks)
                        if let Some(email) = app.current_thread_email() {
                            if let Some(ref message_id) = email.message_id {
                                if let Err(e) = open_email_in_browser(message_id) {
                                    ui_state.set_status(format!("Failed to open browser: {}", e));
                                }
                            } else {
                                ui_state.set_status("Email has no Message-ID".to_string());
                            }
                        }
                    } else {
                        app.enter();
                    }
                }
                KeyCode::Char('g') => {
                    if pending_g {
                        // gg - go to top
                        app.select_first();
                        pending_g = false;
                    } else {
                        // First g - wait for second g
                        pending_g = true;
                    }
                }
                KeyCode::Char('G') => {
                    pending_g = false;
                    app.select_last();
                }
                KeyCode::Char('m') => {
                    app.toggle_group_mode();
                }
                KeyCode::Char('r') => {
                    ui_state.set_busy("Refreshing...");
                    cmd_tx.send(ImapCommand::FetchInbox)?;
                }
                KeyCode::Char('a') => {
                    handle_archive(&mut app, &cmd_tx, &mut ui_state, &mut pending_operation)?;
                }
                KeyCode::Char('A') => {
                    handle_archive_all(&app, &mut ui_state);
                }
                KeyCode::Char('d') => {
                    handle_delete(&mut app, &cmd_tx, &mut ui_state, &mut pending_operation)?;
                }
                KeyCode::Char('D') => {
                    handle_delete_all(&app, &mut ui_state);
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Tracks pending operations so we know what to update when response arrives
enum PendingOp {
    ArchiveSingle(String),
    DeleteSingle(String),
    ArchiveGroup,
    DeleteGroup,
    ArchiveThread(String),
    DeleteThread(String),
}

/// Handles the 'a' key - archive single email (not available in thread view)
fn handle_archive(
    app: &mut App,
    cmd_tx: &mpsc::Sender<ImapCommand>,
    ui_state: &mut UiState,
    pending_operation: &mut Option<PendingOp>,
) -> Result<()> {
    match app.view {
        View::GroupList => {
            // No action on single 'a' in group list
        }
        View::EmailList => {
            // Archive single email (only this sender's email)
            if let Some(email) = app.current_email().cloned() {
                ui_state.set_busy("Archiving...");
                *pending_operation = Some(PendingOp::ArchiveSingle(email.id.clone()));
                cmd_tx.send(ImapCommand::ArchiveEmail(email.id))?;
            }
        }
        View::Thread => {
            // No lowercase 'a' in thread view - use 'A' to archive entire thread
        }
    }
    Ok(())
}

/// Handles the 'A' key - archive all in group
fn handle_archive_all(app: &App, ui_state: &mut UiState) {
    match app.view {
        View::GroupList => {
            // No 'A' in group list view to prevent accidental bulk operations
        }
        View::EmailList => {
            if let Some(group) = app.current_group() {
                let impact = app.current_group_thread_impact();
                ui_state.set_confirm(ConfirmAction::ArchiveEmails {
                    sender: group.key.clone(),
                    count: group.count(),
                    impact,
                });
            }
        }
        View::Thread => {
            // In thread view, 'A' also archives the thread
            let thread_count = app.current_thread_emails().len();
            if thread_count > 0 {
                ui_state.set_confirm(ConfirmAction::ArchiveThread {
                    thread_email_count: thread_count,
                });
            }
        }
    }
}

/// Handles the 'd' key - delete single email (not available in thread view)
fn handle_delete(
    app: &mut App,
    cmd_tx: &mpsc::Sender<ImapCommand>,
    ui_state: &mut UiState,
    pending_operation: &mut Option<PendingOp>,
) -> Result<()> {
    match app.view {
        View::GroupList => {
            // No action on single 'd' in group list
        }
        View::EmailList => {
            // Delete single email (only this sender's email)
            if let Some(email) = app.current_email().cloned() {
                ui_state.set_busy("Deleting...");
                *pending_operation = Some(PendingOp::DeleteSingle(email.id.clone()));
                cmd_tx.send(ImapCommand::DeleteEmail(email.id))?;
            }
        }
        View::Thread => {
            // No lowercase 'd' in thread view - use 'D' to delete entire thread
        }
    }
    Ok(())
}

/// Handles the 'D' key - delete all in group
fn handle_delete_all(app: &App, ui_state: &mut UiState) {
    match app.view {
        View::GroupList => {
            // No 'D' in group list view to prevent accidental bulk operations
        }
        View::EmailList => {
            if let Some(group) = app.current_group() {
                let impact = app.current_group_thread_impact();
                ui_state.set_confirm(ConfirmAction::DeleteEmails {
                    sender: group.key.clone(),
                    count: group.count(),
                    impact,
                });
            }
        }
        View::Thread => {
            // In thread view, 'D' also deletes the thread
            let thread_count = app.current_thread_emails().len();
            if thread_count > 0 {
                ui_state.set_confirm(ConfirmAction::DeleteThread {
                    thread_email_count: thread_count,
                });
            }
        }
    }
}

/// Opens an email in the browser using Gmail's Message-ID search
///
/// Uses the rfc822msgid: search operator to find the specific email.
/// This is the safest approach as it avoids rendering potentially
/// malicious content (unicode exploits, terminal escape sequences) directly in the terminal.
fn open_email_in_browser(message_id: &str) -> Result<()> {
    // URL-encode the message ID for safe inclusion in URL
    // Message-IDs typically look like <unique-id@domain.com>
    let encoded = urlencoding::encode(message_id);
    let url = format!(
        "https://mail.google.com/mail/u/0/#search/rfc822msgid:{}",
        encoded
    );

    // Use platform-specific command to open URL in default browser
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(&url).spawn()?;
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(&url).spawn()?;
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", &url]).spawn()?;
    }

    Ok(())
}

/// Handles a confirmed action
fn handle_confirmed_action(
    app: &mut App,
    cmd_tx: &mpsc::Sender<ImapCommand>,
    ui_state: &mut UiState,
    pending_operation: &mut Option<PendingOp>,
    action: ConfirmAction,
) -> Result<()> {
    match action {
        ConfirmAction::ArchiveEmails { .. } => {
            // Archive only this sender's emails (not full threads)
            let email_ids = app.current_group_email_ids();
            if !email_ids.is_empty() {
                ui_state.set_busy(format!("Archiving {} emails...", email_ids.len()));
                *pending_operation = Some(PendingOp::ArchiveGroup);
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteEmails { .. } => {
            // Delete only this sender's emails (not full threads)
            let email_ids = app.current_group_email_ids();
            if !email_ids.is_empty() {
                ui_state.set_busy(format!("Deleting {} emails...", email_ids.len()));
                *pending_operation = Some(PendingOp::DeleteGroup);
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
        ConfirmAction::ArchiveThread { .. } => {
            // Archive entire thread
            let email_ids = app.current_thread_email_ids();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            if !email_ids.is_empty()
                && let Some(tid) = thread_id
            {
                ui_state.set_busy(format!("Archiving thread ({} emails)...", email_ids.len()));
                *pending_operation = Some(PendingOp::ArchiveThread(tid));
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteThread { .. } => {
            // Delete entire thread
            let email_ids = app.current_thread_email_ids();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            if !email_ids.is_empty()
                && let Some(tid) = thread_id
            {
                ui_state.set_busy(format!("Deleting thread ({} emails)...", email_ids.len()));
                *pending_operation = Some(PendingOp::DeleteThread(tid));
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
    }
    Ok(())
}
