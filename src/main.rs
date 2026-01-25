mod app;
mod config;
mod email;
mod imap_client;
mod ui;

use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::{App, View};
use email::Email;
use imap_client::{EmailClient, ImapClient};
use ui::render::render;
use ui::widgets::{ConfirmAction, UiState};

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

    // Check for credentials
    if !config::has_credentials() {
        eprintln!(
            "Error: Credentials not found.\n\
             Please create a credentials.toml file at {:?}\n\
             with the following format:\n\n\
             email = \"your.email@gmail.com\"\n\
             app_password = \"xxxx xxxx xxxx xxxx\"\n\n\
             Create an App Password at: https://myaccount.google.com/apppasswords",
            config::credentials_path()?
        );
        std::process::exit(1);
    }

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the app
    let result = run_app(&mut terminal);

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

/// Spawns the IMAP worker thread
fn spawn_imap_worker(
    cmd_rx: mpsc::Receiver<ImapCommand>,
    resp_tx: mpsc::Sender<ImapResponse>,
) {
    thread::spawn(move || {
        // Connect to IMAP
        let credentials = match config::load_credentials() {
            Ok(c) => c,
            Err(e) => {
                let _ = resp_tx.send(ImapResponse::Error(format!("Failed to load credentials: {}", e)));
                return;
            }
        };

        let mut client = match ImapClient::connect(&credentials.email, &credentials.app_password) {
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
    });
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::new();
    let mut ui_state = UiState::new();

    // Create channels for IMAP communication
    let (cmd_tx, cmd_rx) = mpsc::channel::<ImapCommand>();
    let (resp_tx, resp_rx) = mpsc::channel::<ImapResponse>();

    // Show connecting status
    ui_state.set_status("Connecting...");
    terminal.draw(|f| render(f, &app, &ui_state))?;

    // Spawn IMAP worker thread
    spawn_imap_worker(cmd_rx, resp_tx);

    // Wait for connection
    loop {
        // Check for responses
        match resp_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ImapResponse::Connected) => {
                ui_state.set_status("Loading emails...");
                terminal.draw(|f| render(f, &app, &ui_state))?;
                cmd_tx.send(ImapCommand::FetchInbox)?;
                break;
            }
            Ok(ImapResponse::Error(e)) => {
                return Err(anyhow::anyhow!("{}", e));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Keep waiting, but check for quit
                if event::poll(Duration::from_millis(0))? {
                    if let Event::Key(key) = event::read()? {
                        if key.code == KeyCode::Char('q') {
                            let _ = cmd_tx.send(ImapCommand::Shutdown);
                            return Ok(());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Track pending operations
    let mut pending_operation: Option<PendingOp> = None;

    // Main event loop
    loop {
        terminal.draw(|f| render(f, &app, &ui_state))?;

        // Check for IMAP responses (non-blocking)
        while let Ok(response) = resp_rx.try_recv() {
            match response {
                ImapResponse::Emails(result) => {
                    match result {
                        Ok(emails) => {
                            app.set_emails(emails);
                            ui_state.clear_status();
                        }
                        Err(e) => {
                            ui_state.set_status(&format!("Error: {}", e));
                        }
                    }
                }
                ImapResponse::ArchiveResult(result) => {
                    if let Some(PendingOp::ArchiveSingle(id)) = pending_operation.take() {
                        if result.is_ok() {
                            app.remove_email(&id);
                        }
                    }
                    ui_state.clear_status();
                }
                ImapResponse::DeleteResult(result) => {
                    if let Some(PendingOp::DeleteSingle(id)) = pending_operation.take() {
                        if result.is_ok() {
                            app.remove_email(&id);
                        }
                    }
                    ui_state.clear_status();
                }
                ImapResponse::MultiArchiveResult(result) => {
                    if let Some(op) = pending_operation.take() {
                        if result.is_ok() {
                            match op {
                                PendingOp::ArchiveGroup => {
                                    app.remove_current_group_emails();
                                }
                                PendingOp::ArchiveThread(thread_id) => {
                                    app.remove_thread(&thread_id);
                                    if app.view == View::ThreadView {
                                        app.exit();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    ui_state.clear_status();
                }
                ImapResponse::MultiDeleteResult(result) => {
                    if let Some(op) = pending_operation.take() {
                        if result.is_ok() {
                            match op {
                                PendingOp::DeleteGroup => {
                                    app.remove_current_group_emails();
                                }
                                PendingOp::DeleteThread(thread_id) => {
                                    app.remove_thread(&thread_id);
                                    if app.view == View::ThreadView {
                                        app.exit();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    ui_state.clear_status();
                }
                _ => {}
            }
        }

        // Poll for keyboard events with timeout
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
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
                    KeyCode::Char('j') | KeyCode::Down => {
                        app.select_next();
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.select_previous();
                    }
                    KeyCode::Enter => {
                        app.enter();
                    }
                    KeyCode::Char('g') => {
                        app.toggle_group_mode();
                    }
                    KeyCode::Char('r') => {
                        ui_state.set_status("Refreshing...");
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
                ui_state.set_status("Archiving...");
                *pending_operation = Some(PendingOp::ArchiveSingle(email.id.clone()));
                cmd_tx.send(ImapCommand::ArchiveEmail(email.id))?;
            }
        }
        View::ThreadView => {
            // No lowercase 'a' in thread view - use 'A' to archive entire thread
        }
    }
    Ok(())
}

/// Handles the 'A' key - archive all in group
fn handle_archive_all(app: &App, ui_state: &mut UiState) {
    match app.view {
        View::GroupList | View::EmailList => {
            if let Some(group) = app.current_group() {
                let impact = app.current_group_thread_impact();
                ui_state.set_confirm(ConfirmAction::ArchiveEmails {
                    sender: group.key.clone(),
                    count: group.count(),
                    impact,
                });
            }
        }
        View::ThreadView => {
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
                ui_state.set_status("Deleting...");
                *pending_operation = Some(PendingOp::DeleteSingle(email.id.clone()));
                cmd_tx.send(ImapCommand::DeleteEmail(email.id))?;
            }
        }
        View::ThreadView => {
            // No lowercase 'd' in thread view - use 'D' to delete entire thread
        }
    }
    Ok(())
}

/// Handles the 'D' key - delete all in group
fn handle_delete_all(app: &App, ui_state: &mut UiState) {
    match app.view {
        View::GroupList | View::EmailList => {
            if let Some(group) = app.current_group() {
                let impact = app.current_group_thread_impact();
                ui_state.set_confirm(ConfirmAction::DeleteEmails {
                    sender: group.key.clone(),
                    count: group.count(),
                    impact,
                });
            }
        }
        View::ThreadView => {
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
                ui_state.set_status(&format!("Archiving {} emails...", email_ids.len()));
                *pending_operation = Some(PendingOp::ArchiveGroup);
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteEmails { .. } => {
            // Delete only this sender's emails (not full threads)
            let email_ids = app.current_group_email_ids();
            if !email_ids.is_empty() {
                ui_state.set_status(&format!("Deleting {} emails...", email_ids.len()));
                *pending_operation = Some(PendingOp::DeleteGroup);
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
        ConfirmAction::ArchiveThread { .. } => {
            // Archive entire thread
            let email_ids = app.current_thread_email_ids();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            if !email_ids.is_empty() {
                if let Some(tid) = thread_id {
                    ui_state.set_status(&format!("Archiving thread ({} emails)...", email_ids.len()));
                    *pending_operation = Some(PendingOp::ArchiveThread(tid));
                    cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
                }
            }
        }
        ConfirmAction::DeleteThread { .. } => {
            // Delete entire thread
            let email_ids = app.current_thread_email_ids();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            if !email_ids.is_empty() {
                if let Some(tid) = thread_id {
                    ui_state.set_status(&format!("Deleting thread ({} emails)...", email_ids.len()));
                    *pending_operation = Some(PendingOp::DeleteThread(tid));
                    cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
                }
            }
        }
    }
    Ok(())
}
