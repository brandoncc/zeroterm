mod app;
mod config;
#[macro_use]
mod debug;
mod demo;
mod email;
mod imap_client;
mod ui;

use std::collections::HashMap;
use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use app::{App, UndoActionType, UndoContext, UndoEntry, View};
use config::AccountConfig;
use email::Email;
use imap_client::{EmailClient, ImapClient};
use ui::render::{render, render_account_select};
use ui::widgets::{AccountSelection, ConfirmAction, TextViewState, UiState};

/// Commands sent to the IMAP worker thread
enum ImapCommand {
    FetchInbox {
        parallel_connections: usize,
    },
    ArchiveMultiple(Vec<(String, String)>), // Vec<(uid, folder)>
    DeleteMultiple(Vec<(String, String)>),  // Vec<(uid, folder)>
    /// Vec<(message_id, dest_uid, current_folder, dest_folder)>
    /// dest_uid is used for fast restore if available, falls back to Message-ID search
    RestoreEmails(Vec<(Option<String>, Option<u32>, String, String)>),
    /// Fetch email body (uid, folder)
    FetchBody {
        uid: String,
        folder: String,
    },
    Shutdown,
}

/// Responses from the IMAP worker thread
enum ImapResponse {
    Emails(Result<Vec<Email>>),
    /// Multi-archive result with source UID -> dest UID mapping from COPYUID
    MultiArchiveResult(Result<HashMap<String, u32>>),
    /// Multi-delete result with source UID -> dest UID mapping from COPYUID
    MultiDeleteResult(Result<HashMap<String, u32>>),
    RestoreResult(Result<()>),
    /// Email body fetch result with UID
    BodyResult {
        uid: String,
        result: Result<String>,
    },
    /// Progress update during bulk operations (current, total, action)
    Progress(usize, usize, String),
    /// Retry status update (attempt number, max attempts, operation description)
    Retrying {
        attempt: u32,
        max_attempts: u32,
        action: String,
    },
    Connected,
    Error(String),
}

fn print_help() {
    println!(
        "\
zeroterm {}
Terminal-based email client for achieving inbox zero

USAGE:
    zeroterm [OPTIONS]

OPTIONS:
    -h, --help       Print help information
    -V, --version    Print version information
        --demo       Run in demo mode with fake data
        --debug      Enable debug logging

NAVIGATION:
    j/k              Move down/up in lists
    Enter            Select group or email / view email body
    Escape           Go back to previous view / clear filter
    /                Filter emails (in email list view)
    q                Quit

ACTIONS:
    a                Archive email (in group/email view)
    d                Delete email (in group/email view)
    A                Archive all emails from sender (with confirmation)
    D                Delete all emails from sender (with confirmation)
    e                Open email in browser (Gmail)
    u                Undo last action

CONFIG:
    Configuration file location: ~/.config/zeroterm/config.toml

    Example config:
        # Global options (all optional)
        protect_threads = true       # Require confirmation for bulk actions (default: true)
        parallel_connections = 5     # IMAP connections for loading (default: 5)
        debug = false                # Enable debug logging (default: false)

        [accounts.personal]
        backend = \"gmail\"
        email = \"your.email@gmail.com\"
        app_password = \"xxxx xxxx xxxx xxxx\"

    The app_password can be a plain string or a 1Password reference (op://vault/item/field).
    Create an App Password at: https://myaccount.google.com/apppasswords",
        env!("CARGO_PKG_VERSION")
    );
}

fn main() -> Result<()> {
    // Check for help flag
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    // Check for version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("zeroterm {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Check for demo mode
    let demo_mode = std::env::args().any(|arg| arg == "--demo");
    let debug_flag = std::env::args().any(|arg| arg == "--debug");

    if demo_mode {
        // Initialize debug logging for demo mode too
        debug::init(debug_flag);
        return run_demo_mode();
    }

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

    // Initialize debug logging (--debug flag overrides config)
    debug::init(debug_flag || cfg.debug);
    debug_log!("Zeroterm starting up");

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
        run_app(
            &mut terminal,
            account,
            cfg.parallel_connections,
            cfg.advance_on_select,
        )
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

/// Runs the application in demo mode with fake data
fn run_demo_mode() -> Result<()> {
    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_demo_app(&mut terminal);

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

/// Storage for emails removed in demo mode (for undo support)
/// Each entry corresponds to an undo history entry at the same index
struct DemoUndoStorage {
    /// Removed emails stored by undo entry index
    /// We use a Vec because undo entries are added at index 0 (newest first)
    emails: Vec<Vec<Email>>,
}

/// Simulated latency for demo mode operations (milliseconds)
const DEMO_LATENCY_MS: u64 = 300;

/// Pending operations in demo mode (mirrors PendingOp for real mode)
enum DemoPendingOp {
    ArchiveGroup {
        emails: Vec<Email>,
        sender: String,
    },
    DeleteGroup {
        emails: Vec<Email>,
        sender: String,
    },
    ArchiveThread {
        thread_id: String,
        thread_emails: Vec<Email>,
        subject: String,
    },
    DeleteThread {
        thread_id: String,
        thread_emails: Vec<Email>,
        subject: String,
    },
    ArchiveSelected {
        emails: Vec<Email>,
        count: usize,
        processed: usize,
        processed_ids: Vec<String>,
    },
    DeleteSelected {
        emails: Vec<Email>,
        count: usize,
        processed: usize,
        processed_ids: Vec<String>,
    },
    Undo {
        index: usize,
        emails: Vec<Email>,
    },
}

impl DemoPendingOp {
    /// Returns the busy message to display while this operation is pending
    fn busy_message(&self) -> &'static str {
        match self {
            DemoPendingOp::ArchiveGroup { .. }
            | DemoPendingOp::ArchiveThread { .. }
            | DemoPendingOp::ArchiveSelected { .. } => "Archiving...",
            DemoPendingOp::DeleteGroup { .. }
            | DemoPendingOp::DeleteThread { .. }
            | DemoPendingOp::DeleteSelected { .. } => "Deleting...",
            DemoPendingOp::Undo { .. } => "Restoring...",
        }
    }
}

impl DemoUndoStorage {
    fn new() -> Self {
        Self { emails: Vec::new() }
    }

    /// Store emails for a new undo entry (inserted at front, matching undo history)
    fn push(&mut self, emails: Vec<Email>) {
        self.emails.insert(0, emails);
        // Keep in sync with MAX_UNDO_HISTORY (50)
        if self.emails.len() > 50 {
            self.emails.truncate(50);
        }
    }

    /// Remove and return emails at the given index
    fn remove(&mut self, index: usize) -> Option<Vec<Email>> {
        if index < self.emails.len() {
            Some(self.emails.remove(index))
        } else {
            None
        }
    }
}

/// The demo app event loop - no IMAP, all actions are simulated
fn run_demo_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::new();
    app.set_user_email("demo@example.com".to_string());
    app.set_emails(demo::create_demo_emails());
    let mut ui_state = UiState::new();
    let mut undo_storage = DemoUndoStorage::new();

    // Track pending 'g' for gg sequence
    let mut pending_g = false;

    // Pending operation for simulated network latency
    let mut pending_op: Option<DemoPendingOp> = None;
    let mut op_start_time: Option<Instant> = None;

    // Demo mode uses the default setting for advance_on_select
    let advance_on_select = true;

    // Main event loop
    loop {
        // Check if pending operation should complete
        if let (Some(op), Some(start)) = (pending_op.take(), op_start_time.take()) {
            if start.elapsed() >= Duration::from_millis(DEMO_LATENCY_MS) {
                // Execute the operation (may return a continuation for multi-step operations)
                if let Some(continuation) =
                    execute_demo_op(&mut app, &mut ui_state, &mut undo_storage, op)
                {
                    pending_op = Some(continuation);
                    op_start_time = Some(Instant::now());
                }
            } else {
                // Not ready yet, put it back
                pending_op = Some(op);
                op_start_time = Some(start);
            }
        }

        app.ensure_valid_selection();
        terminal.draw(|f| render(f, &app, &mut ui_state))?;

        // Poll for keyboard events with timeout
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Handle confirmation dialog input
            if ui_state.is_confirming() {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        if let Some(action) = ui_state.confirm_action.take() {
                            if matches!(action, ConfirmAction::Quit) {
                                break;
                            }
                            if let Some(op) = handle_demo_confirmed_action(&app, action) {
                                // For selected emails, record undo entry and show "1 of N" progress
                                match &op {
                                    DemoPendingOp::ArchiveSelected { emails, count, .. } => {
                                        // Record undo entry upfront (demo mode doesn't have real UIDs)
                                        let undo_emails: Vec<(
                                            Option<String>,
                                            Option<u32>,
                                            String,
                                        )> = emails
                                            .iter()
                                            .map(|e| {
                                                (
                                                    e.message_id.clone(),
                                                    None,
                                                    e.source_folder.clone(),
                                                )
                                            })
                                            .collect();
                                        if !undo_emails.is_empty() {
                                            let undo_entry = UndoEntry {
                                                action_type: UndoActionType::Archive,
                                                context: UndoContext::Group {
                                                    sender: format!("{} selected", count),
                                                },
                                                emails: undo_emails,
                                                current_folder: "[Gmail]/All Mail".to_string(),
                                            };
                                            undo_storage.push(emails.clone());
                                            app.push_undo(undo_entry);
                                        }
                                        ui_state.set_busy(format!("Archiving 1 of {}...", count));
                                    }
                                    DemoPendingOp::DeleteSelected { emails, count, .. } => {
                                        // Record undo entry upfront (demo mode doesn't have real UIDs)
                                        let undo_emails: Vec<(
                                            Option<String>,
                                            Option<u32>,
                                            String,
                                        )> = emails
                                            .iter()
                                            .map(|e| {
                                                (
                                                    e.message_id.clone(),
                                                    None,
                                                    e.source_folder.clone(),
                                                )
                                            })
                                            .collect();
                                        if !undo_emails.is_empty() {
                                            let undo_entry = UndoEntry {
                                                action_type: UndoActionType::Delete,
                                                context: UndoContext::Group {
                                                    sender: format!("{} selected", count),
                                                },
                                                emails: undo_emails,
                                                current_folder: "[Gmail]/Trash".to_string(),
                                            };
                                            undo_storage.push(emails.clone());
                                            app.push_undo(undo_entry);
                                        }
                                        ui_state.set_busy(format!("Deleting 1 of {}...", count));
                                    }
                                    _ => {
                                        ui_state.set_busy(op.busy_message());
                                    }
                                }
                                pending_op = Some(op);
                                op_start_time = Some(Instant::now());
                            }
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        ui_state.clear_confirm();
                    }
                    _ => {}
                }
                continue;
            }

            // Clear status message on any key press
            if ui_state.has_status() {
                ui_state.clear_status();
                continue;
            }

            // Handle help menu
            if ui_state.is_showing_help() {
                match key.code {
                    KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                        ui_state.hide_help();
                    }
                    _ => {}
                }
                continue;
            }

            // Handle filter input mode (only in EmailList view)
            if ui_state.is_filter_input_active() {
                match key.code {
                    KeyCode::Esc => {
                        // Clear filter entirely and exit input mode
                        app.clear_text_filter();
                        ui_state.clear_filter_query();
                        ui_state.exit_filter_input_mode();
                    }
                    KeyCode::Enter => {
                        // Exit input mode, keep filter active
                        ui_state.exit_filter_input_mode();
                    }
                    KeyCode::Backspace => {
                        ui_state.backspace_filter();
                        // Update filter in real-time
                        let query = ui_state.filter_query().to_string();
                        if query.is_empty() {
                            app.clear_text_filter();
                        } else {
                            app.set_text_filter(Some(query));
                        }
                    }
                    KeyCode::Char(c) => {
                        ui_state.append_filter_char(c);
                        // Update filter in real-time
                        let query = ui_state.filter_query().to_string();
                        app.set_text_filter(Some(query));
                    }
                    _ => {}
                }
                continue;
            }

            // Toggle help menu with ?
            if key.code == KeyCode::Char('?') {
                ui_state.show_help();
                continue;
            }

            // Enter filter mode with / (only in EmailList view)
            if key.code == KeyCode::Char('/') && app.view == View::EmailList {
                if app.has_text_filter() {
                    // Re-enter input mode with existing query
                    ui_state.enter_filter_input_mode_with_query(app.text_filter().unwrap_or(""));
                } else {
                    // Enter fresh filter input mode
                    ui_state.enter_filter_input_mode();
                }
                continue;
            }

            // Clear pending g for any key that's not part of the gg sequence
            let is_g_sequence = matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G'));
            if !is_g_sequence {
                pending_g = false;
            }

            // Handle UndoHistory view separately
            if app.view == View::UndoHistory {
                match key.code {
                    KeyCode::Char('q') => {
                        ui_state.set_confirm(ConfirmAction::Quit);
                    }
                    KeyCode::Esc => {
                        app.exit_undo_history();
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        app.select_next();
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.select_previous();
                    }
                    KeyCode::Char('g') => {
                        if pending_g {
                            app.select_first();
                            pending_g = false;
                        } else {
                            pending_g = true;
                        }
                    }
                    KeyCode::Char('G') => {
                        pending_g = false;
                        app.select_last();
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let half_page = ui_state.viewport_heights.for_view(app.view) / 2;
                        app.select_next_n(half_page.max(1));
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let half_page = ui_state.viewport_heights.for_view(app.view) / 2;
                        app.select_previous_n(half_page.max(1));
                    }
                    KeyCode::Enter => {
                        // Execute undo in demo mode
                        if let Some(op) = handle_demo_undo(&app, &mut undo_storage) {
                            ui_state.set_busy(op.busy_message());
                            pending_op = Some(op);
                            op_start_time = Some(Instant::now());
                        }
                    }
                    _ => {}
                }
                continue;
            }

            // Handle TextView separately (demo mode)
            if app.view == View::EmailBody {
                match key.code {
                    KeyCode::Char('q') => {
                        ui_state.set_confirm(ConfirmAction::Quit);
                    }
                    KeyCode::Esc => {
                        app.exit_text_view();
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        app.scroll_text_view_down(1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.scroll_text_view_up(1);
                    }
                    KeyCode::Char('g') => {
                        if pending_g {
                            app.text_view_scroll = 0;
                            pending_g = false;
                        } else {
                            pending_g = true;
                        }
                    }
                    KeyCode::Char('G') => {
                        pending_g = false;
                        app.text_view_scroll = usize::MAX;
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let half_page = ui_state.viewport_heights.text_view / 2;
                        app.scroll_text_view_down(half_page.max(1));
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let half_page = ui_state.viewport_heights.text_view / 2;
                        app.scroll_text_view_up(half_page.max(1));
                    }
                    KeyCode::Char('e') => {
                        ui_state.set_status("Demo mode: would open email in browser".to_string());
                    }
                    KeyCode::Char('?') => {
                        ui_state.show_help();
                    }
                    _ => {}
                }
                continue;
            }

            // Normal input handling
            match key.code {
                KeyCode::Char('q') => {
                    ui_state.set_confirm(ConfirmAction::Quit);
                }
                KeyCode::Esc => {
                    // In EmailList view with active filter, clear filter instead of exiting
                    if app.view == View::EmailList && app.has_text_filter() {
                        app.clear_text_filter();
                        ui_state.clear_filter_query();
                    } else if app.view != View::GroupList {
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
                        // Enter text view for the selected email in thread (demo mode)
                        if let Some(email) = app.current_thread_email() {
                            let email_id = email.id.clone();
                            let from = email.from.clone();
                            let subject = email.subject.clone();
                            app.enter_text_view(&email_id);
                            // In demo mode, simulate having a body already loaded
                            let body = format!(
                                "This is a demo email body.\n\n\
                                In real mode, the actual email content would be fetched from the server.\n\n\
                                From: {}\n\
                                Subject: {}\n\n\
                                Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                                Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.",
                                from, subject
                            );
                            ui_state.text_view_state = TextViewState::Loaded(body);
                        }
                    } else if app.view == View::EmailList
                        && !app.current_email_is_multi_message_thread()
                    {
                        // Single email - enter text view directly (demo mode)
                        if let Some(email) = app.current_email() {
                            let email_id = email.id.clone();
                            let from = email.from.clone();
                            let subject = email.subject.clone();
                            app.enter_text_view(&email_id);
                            // In demo mode, simulate having a body already loaded
                            let body = format!(
                                "This is a demo email body.\n\n\
                                In real mode, the actual email content would be fetched from the server.\n\n\
                                From: {}\n\
                                Subject: {}\n\n\
                                Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                                Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.",
                                from, subject
                            );
                            ui_state.text_view_state = TextViewState::Loaded(body);
                        }
                    } else {
                        app.enter();
                    }
                }
                KeyCode::Char('e') => {
                    // Open in browser (demo mode)
                    if matches!(app.view, View::Thread | View::EmailList) {
                        ui_state.set_status("Demo mode: would open email in browser".to_string());
                    }
                }
                KeyCode::Char('g') => {
                    if pending_g {
                        app.select_first();
                        pending_g = false;
                    } else {
                        pending_g = true;
                    }
                }
                KeyCode::Char('G') => {
                    pending_g = false;
                    app.select_last();
                }
                KeyCode::Char('m') => {
                    if app.view == View::GroupList {
                        app.toggle_group_mode();
                    }
                }
                KeyCode::Char('r') => {
                    ui_state.set_status("Demo mode: refresh simulated".to_string());
                }
                KeyCode::Char('t') => {
                    if app.view == View::GroupList || app.view == View::EmailList {
                        app.toggle_thread_filter();
                    }
                }
                KeyCode::Char('u') => {
                    app.enter_undo_history();
                }
                KeyCode::Char('a') => {
                    if let Some(op) = handle_demo_archive(&app, &mut ui_state) {
                        ui_state.set_busy(op.busy_message());
                        pending_op = Some(op);
                        op_start_time = Some(Instant::now());
                    }
                }
                KeyCode::Char('A') => {
                    handle_demo_archive_all(&app, &mut ui_state);
                }
                KeyCode::Char('d') => {
                    if let Some(op) = handle_demo_delete(&app, &mut ui_state) {
                        ui_state.set_busy(op.busy_message());
                        pending_op = Some(op);
                        op_start_time = Some(Instant::now());
                    }
                }
                KeyCode::Char('D') => {
                    handle_demo_delete_all(&app, &mut ui_state);
                }
                KeyCode::Char(' ') => {
                    if app.view == View::Thread {
                        ui_state.set_status(
                            "Cannot select individual emails in thread view. Press Enter to open the thread."
                                .to_string(),
                        );
                    } else if app.view == View::EmailList
                        && let app::SelectionResult::Toggled = app.toggle_email_selection()
                        && advance_on_select
                    {
                        app.select_next();
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Executes a pending demo operation after simulated latency.
/// Returns Some(op) if there's more work to do (for multi-step operations like selected emails).
fn execute_demo_op(
    app: &mut App,
    ui_state: &mut UiState,
    undo_storage: &mut DemoUndoStorage,
    op: DemoPendingOp,
) -> Option<DemoPendingOp> {
    match op {
        DemoPendingOp::ArchiveGroup { emails, sender } => {
            ui_state.clear_busy();
            // Demo mode doesn't have real destination UIDs, so we use None
            let undo_emails: Vec<(Option<String>, Option<u32>, String)> = emails
                .iter()
                .map(|e| (e.message_id.clone(), None, e.source_folder.clone()))
                .collect();
            if !undo_emails.is_empty() {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Archive,
                    context: UndoContext::Group { sender },
                    emails: undo_emails,
                    current_folder: "[Gmail]/All Mail".to_string(),
                };
                undo_storage.push(emails);
                app.push_undo(undo_entry);
            }
            // Remove all emails from threads touched by this group
            app.remove_current_group_threads();
            None
        }
        DemoPendingOp::DeleteGroup { emails, sender } => {
            ui_state.clear_busy();
            // Demo mode doesn't have real destination UIDs, so we use None
            let undo_emails: Vec<(Option<String>, Option<u32>, String)> = emails
                .iter()
                .map(|e| (e.message_id.clone(), None, e.source_folder.clone()))
                .collect();
            if !undo_emails.is_empty() {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Delete,
                    context: UndoContext::Group { sender },
                    emails: undo_emails,
                    current_folder: "[Gmail]/Trash".to_string(),
                };
                undo_storage.push(emails);
                app.push_undo(undo_entry);
            }
            // Remove all emails from threads touched by this group
            app.remove_current_group_threads();
            None
        }
        DemoPendingOp::ArchiveThread {
            thread_id,
            thread_emails,
            subject,
        } => {
            ui_state.clear_busy();
            // Demo mode doesn't have real destination UIDs, so we use None
            let undo_emails: Vec<(Option<String>, Option<u32>, String)> = thread_emails
                .iter()
                .map(|e| (e.message_id.clone(), None, e.source_folder.clone()))
                .collect();
            if !undo_emails.is_empty() {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Archive,
                    context: UndoContext::Thread { subject },
                    emails: undo_emails,
                    current_folder: "[Gmail]/All Mail".to_string(),
                };
                undo_storage.push(thread_emails);
                app.push_undo(undo_entry);
            }
            app.remove_thread(&thread_id);
            if app.view == View::Thread {
                app.exit();
            }
            None
        }
        DemoPendingOp::DeleteThread {
            thread_id,
            thread_emails,
            subject,
        } => {
            ui_state.clear_busy();
            // Demo mode doesn't have real destination UIDs, so we use None
            let undo_emails: Vec<(Option<String>, Option<u32>, String)> = thread_emails
                .iter()
                .map(|e| (e.message_id.clone(), None, e.source_folder.clone()))
                .collect();
            if !undo_emails.is_empty() {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Delete,
                    context: UndoContext::Thread { subject },
                    emails: undo_emails,
                    current_folder: "[Gmail]/Trash".to_string(),
                };
                undo_storage.push(thread_emails);
                app.push_undo(undo_entry);
            }
            app.remove_thread(&thread_id);
            if app.view == View::Thread {
                app.exit();
            }
            None
        }
        DemoPendingOp::ArchiveSelected {
            mut emails,
            count,
            processed,
            mut processed_ids,
        } => {
            // Process one email per cycle for progress display
            if let Some(email) = emails.first().cloned() {
                app.remove_email(&email.id);
                processed_ids.push(email.id.clone());
                emails.remove(0);
                let new_processed = processed + 1;

                if emails.is_empty() {
                    // All done - deselect only processed emails (preserve hidden selections)
                    ui_state.clear_busy();
                    app.deselect_emails(&processed_ids);
                    None
                } else {
                    // More to process - update progress and continue
                    ui_state.set_busy(format!("Archiving {} of {}...", new_processed + 1, count));
                    Some(DemoPendingOp::ArchiveSelected {
                        emails,
                        count,
                        processed: new_processed,
                        processed_ids,
                    })
                }
            } else {
                ui_state.clear_busy();
                app.deselect_emails(&processed_ids);
                None
            }
        }
        DemoPendingOp::DeleteSelected {
            mut emails,
            count,
            processed,
            mut processed_ids,
        } => {
            // Process one email per cycle for progress display
            if let Some(email) = emails.first().cloned() {
                app.remove_email(&email.id);
                processed_ids.push(email.id.clone());
                emails.remove(0);
                let new_processed = processed + 1;

                if emails.is_empty() {
                    // All done - deselect only processed emails (preserve hidden selections)
                    ui_state.clear_busy();
                    app.deselect_emails(&processed_ids);
                    None
                } else {
                    // More to process - update progress and continue
                    ui_state.set_busy(format!("Deleting {} of {}...", new_processed + 1, count));
                    Some(DemoPendingOp::DeleteSelected {
                        emails,
                        count,
                        processed: new_processed,
                        processed_ids,
                    })
                }
            } else {
                ui_state.clear_busy();
                app.deselect_emails(&processed_ids);
                None
            }
        }
        DemoPendingOp::Undo { index, emails } => {
            ui_state.clear_busy();
            app.restore_emails(emails);
            app.pop_undo(index);
            None
        }
    }
}

/// Handles 'a' key in demo mode - returns pending operation if action should proceed
fn handle_demo_archive(app: &App, ui_state: &mut UiState) -> Option<DemoPendingOp> {
    match app.view {
        View::GroupList | View::UndoHistory | View::EmailBody => None,
        View::EmailList => {
            // Check if there are selected emails - require confirmation
            if app.has_selection() {
                let count = app.selected_thread_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::ArchiveSelected { count });
                }
                return None;
            }

            // No selection - archive the entire thread
            let thread_emails: Vec<Email> =
                app.current_thread_emails().into_iter().cloned().collect();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            let subject = app.current_email().map(|e| e.subject.clone());
            if let (Some(tid), Some(subj)) = (thread_id, subject)
                && !thread_emails.is_empty()
            {
                return Some(DemoPendingOp::ArchiveThread {
                    thread_id: tid,
                    thread_emails,
                    subject: subj,
                });
            }
            None
        }
        View::Thread => None,
    }
}

/// Handles 'A' key in demo mode
fn handle_demo_archive_all(app: &App, ui_state: &mut UiState) {
    match app.view {
        View::GroupList | View::UndoHistory | View::EmailBody => {}
        View::EmailList => {
            // If there are selected emails, archive those threads
            if app.has_selection() {
                let count = app.selected_thread_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::ArchiveSelected { count });
                }
                return;
            }

            // Archive all threads touched by this group's emails
            if let Some(group) = app.current_group() {
                ui_state.set_confirm(ConfirmAction::ArchiveEmails {
                    sender: group.key.clone(),
                    count: app.current_group_thread_email_ids().len(),
                    filtered: false,
                });
            }
        }
        View::Thread => {
            let thread_count = app.current_thread_emails().len();
            if thread_count > 0 {
                ui_state.set_confirm(ConfirmAction::ArchiveThread {
                    thread_email_count: thread_count,
                });
            }
        }
    }
}

/// Handles 'd' key in demo mode - returns pending operation if action should proceed
fn handle_demo_delete(app: &App, ui_state: &mut UiState) -> Option<DemoPendingOp> {
    match app.view {
        View::GroupList | View::UndoHistory | View::EmailBody => None,
        View::EmailList => {
            // Check if there are selected emails - require confirmation
            if app.has_selection() {
                let count = app.selected_thread_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::DeleteSelected { count });
                }
                return None;
            }

            // No selection - delete the entire thread
            let thread_emails: Vec<Email> =
                app.current_thread_emails().into_iter().cloned().collect();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            let subject = app.current_email().map(|e| e.subject.clone());
            if let (Some(tid), Some(subj)) = (thread_id, subject)
                && !thread_emails.is_empty()
            {
                return Some(DemoPendingOp::DeleteThread {
                    thread_id: tid,
                    thread_emails,
                    subject: subj,
                });
            }
            None
        }
        View::Thread => None,
    }
}

/// Handles 'D' key in demo mode
fn handle_demo_delete_all(app: &App, ui_state: &mut UiState) {
    match app.view {
        View::GroupList | View::UndoHistory | View::EmailBody => {}
        View::EmailList => {
            // If there are selected emails, delete those threads
            if app.has_selection() {
                let count = app.selected_thread_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::DeleteSelected { count });
                }
                return;
            }

            // Delete all threads touched by this group's emails
            if let Some(group) = app.current_group() {
                ui_state.set_confirm(ConfirmAction::DeleteEmails {
                    sender: group.key.clone(),
                    count: app.current_group_thread_email_ids().len(),
                    filtered: false,
                });
            }
        }
        View::Thread => {
            let thread_count = app.current_thread_emails().len();
            if thread_count > 0 {
                ui_state.set_confirm(ConfirmAction::DeleteThread {
                    thread_email_count: thread_count,
                });
            }
        }
    }
}

/// Handles undo in demo mode - returns pending operation if action should proceed
fn handle_demo_undo(app: &App, undo_storage: &mut DemoUndoStorage) -> Option<DemoPendingOp> {
    let selected_idx = app.selected_undo;
    undo_storage
        .remove(selected_idx)
        .map(|emails| DemoPendingOp::Undo {
            index: selected_idx,
            emails,
        })
}

/// Handles confirmed actions in demo mode - returns pending operation
fn handle_demo_confirmed_action(app: &App, action: ConfirmAction) -> Option<DemoPendingOp> {
    match action {
        ConfirmAction::ArchiveEmails { sender, .. } => {
            // Get all emails from threads touched by this group
            let emails = app.current_group_thread_emails_cloned();
            if emails.is_empty() {
                None
            } else {
                Some(DemoPendingOp::ArchiveGroup { emails, sender })
            }
        }
        ConfirmAction::DeleteEmails { sender, .. } => {
            // Get all emails from threads touched by this group
            let emails = app.current_group_thread_emails_cloned();
            if emails.is_empty() {
                None
            } else {
                Some(DemoPendingOp::DeleteGroup { emails, sender })
            }
        }
        ConfirmAction::ArchiveThread { .. } => app.current_email().map(|email| {
            let thread_emails: Vec<Email> = app
                .current_thread_emails()
                .iter()
                .map(|e| (*e).clone())
                .collect();
            DemoPendingOp::ArchiveThread {
                thread_id: email.thread_id.clone(),
                thread_emails,
                subject: email.subject.clone(),
            }
        }),
        ConfirmAction::DeleteThread { .. } => app.current_email().map(|email| {
            let thread_emails: Vec<Email> = app
                .current_thread_emails()
                .iter()
                .map(|e| (*e).clone())
                .collect();
            DemoPendingOp::DeleteThread {
                thread_id: email.thread_id.clone(),
                thread_emails,
                subject: email.subject.clone(),
            }
        }),
        ConfirmAction::ArchiveSelected { count } => {
            // Get all emails from threads touched by selected emails
            let emails = app.selected_thread_emails_cloned();
            if !emails.is_empty() {
                Some(DemoPendingOp::ArchiveSelected {
                    emails,
                    count,
                    processed: 0,
                    processed_ids: Vec::new(),
                })
            } else {
                None
            }
        }
        ConfirmAction::DeleteSelected { count } => {
            // Get all emails from threads touched by selected emails
            let emails = app.selected_thread_emails_cloned();
            if !emails.is_empty() {
                Some(DemoPendingOp::DeleteSelected {
                    emails,
                    count,
                    processed: 0,
                    processed_ids: Vec::new(),
                })
            } else {
                None
            }
        }
        ConfirmAction::Quit => unreachable!(),
    }
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

/// Maximum number of retry attempts for IMAP operations
const MAX_RETRIES: u32 = 3;
/// Initial backoff delay in milliseconds (doubles with each retry)
const INITIAL_BACKOFF_MS: u64 = 100;

/// Retries an operation with exponential backoff
/// Calls `on_retry` before each retry attempt with the attempt number
fn retry_with_backoff<T, F, R>(mut operation: F, mut on_retry: R) -> Result<T>
where
    F: FnMut() -> Result<T>,
    R: FnMut(u32),
{
    let mut last_error = None;

    for attempt in 0..MAX_RETRIES {
        match operation() {
            Ok(result) => return Ok(result),
            Err(e) => {
                debug_log!(
                    "Operation failed (attempt {}/{}): {}",
                    attempt + 1,
                    MAX_RETRIES,
                    e
                );
                last_error = Some(e);
                if attempt < MAX_RETRIES - 1 {
                    on_retry(attempt + 1);
                    let backoff_ms = INITIAL_BACKOFF_MS * (1 << attempt);
                    debug_log!("Retrying after {}ms backoff", backoff_ms);
                    thread::sleep(Duration::from_millis(backoff_ms));
                }
            }
        }
    }

    Err(last_error.unwrap())
}

/// Retries an operation with exponential backoff (silent version for worker threads)
fn retry_silent<T, F>(mut operation: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut last_error = None;

    for attempt in 0..MAX_RETRIES {
        match operation() {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = Some(e);
                if attempt < MAX_RETRIES - 1 {
                    let backoff_ms = INITIAL_BACKOFF_MS * (1 << attempt);
                    thread::sleep(Duration::from_millis(backoff_ms));
                }
            }
        }
    }

    Err(last_error.unwrap())
}

/// Spawns the IMAP worker thread
fn spawn_imap_worker(
    cmd_rx: mpsc::Receiver<ImapCommand>,
    resp_tx: mpsc::Sender<ImapResponse>,
    account: AccountConfig,
) {
    thread::spawn(move || {
        debug_log!("IMAP worker: connecting to {}", account.email);
        let mut client = match ImapClient::connect(&account.email, &account.app_password) {
            Ok(c) => {
                debug_log!("IMAP worker: connected successfully");
                c
            }
            Err(e) => {
                debug_log!("IMAP worker: connection failed: {}", e);
                let _ = resp_tx.send(ImapResponse::Error(format!("Failed to connect: {}", e)));
                return;
            }
        };

        let _ = resp_tx.send(ImapResponse::Connected);

        // Process commands
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                ImapCommand::FetchInbox {
                    parallel_connections,
                } => {
                    debug_log!(
                        "FetchInbox: starting with {} parallel connections",
                        parallel_connections
                    );
                    let fetch_start = Instant::now();

                    // Get message counts first (with retry)
                    let resp_tx_retry = resp_tx.clone();
                    let inbox_count = match retry_with_backoff(
                        || client.get_folder_count("INBOX"),
                        |attempt| {
                            let _ = resp_tx_retry.send(ImapResponse::Retrying {
                                attempt,
                                max_attempts: MAX_RETRIES,
                                action: "fetch".to_string(),
                            });
                        },
                    ) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = resp_tx.send(ImapResponse::Emails(Err(e)));
                            continue;
                        }
                    };
                    let resp_tx_retry = resp_tx.clone();
                    let sent_count = match retry_with_backoff(
                        || client.get_folder_count("[Gmail]/Sent Mail"),
                        |attempt| {
                            let _ = resp_tx_retry.send(ImapResponse::Retrying {
                                attempt,
                                max_attempts: MAX_RETRIES,
                                action: "fetch".to_string(),
                            });
                        },
                    ) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = resp_tx.send(ImapResponse::Emails(Err(e)));
                            continue;
                        }
                    };

                    let total = (inbox_count + sent_count) as usize;
                    debug_log!(
                        "FetchInbox: found {} inbox + {} sent = {} total emails",
                        inbox_count,
                        sent_count,
                        total
                    );

                    // If mailbox is empty or small, use sequential fetch
                    if total == 0 {
                        debug_log!("FetchInbox: no emails to fetch");
                        let _ = resp_tx.send(ImapResponse::Emails(Ok(Vec::new())));
                        continue;
                    }

                    // Shared counter for progress reporting
                    let fetched_count = Arc::new(AtomicUsize::new(0));

                    // Calculate how many workers we actually need
                    let num_workers = parallel_connections.min(total).max(1);

                    // Spawn progress reporting thread
                    let progress_fetched = Arc::clone(&fetched_count);
                    let progress_tx = resp_tx.clone();
                    let progress_handle = thread::spawn(move || {
                        loop {
                            let current = progress_fetched.load(Ordering::Relaxed);
                            let _ = progress_tx.send(ImapResponse::Progress(
                                current,
                                total,
                                "Loading".to_string(),
                            ));
                            if current >= total {
                                break;
                            }
                            thread::sleep(Duration::from_millis(100));
                        }
                    });

                    // Spawn parallel inbox fetchers
                    let inbox_chunk_size = if inbox_count > 0 {
                        (inbox_count as usize).div_ceil(num_workers)
                    } else {
                        0
                    };

                    let inbox_handles: Vec<_> = (0..num_workers)
                        .filter_map(|i| {
                            let start = (i * inbox_chunk_size + 1) as u32;
                            let end = (((i + 1) * inbox_chunk_size) as u32).min(inbox_count);
                            if start > inbox_count || inbox_count == 0 {
                                return None;
                            }
                            let email_addr = account.email.clone();
                            let password = account.app_password.clone();
                            let counter = Arc::clone(&fetched_count);
                            Some(thread::spawn(move || {
                                retry_silent(|| {
                                    let mut worker_client =
                                        ImapClient::connect(&email_addr, &password)?;
                                    let emails = worker_client.fetch_inbox_range(
                                        start,
                                        end,
                                        Some(&counter),
                                    )?;
                                    let _ = worker_client.logout();
                                    Ok::<_, anyhow::Error>(emails)
                                })
                            }))
                        })
                        .collect();

                    // Spawn parallel sent fetchers
                    let sent_chunk_size = if sent_count > 0 {
                        (sent_count as usize).div_ceil(num_workers)
                    } else {
                        0
                    };

                    let sent_handles: Vec<_> = (0..num_workers)
                        .filter_map(|i| {
                            let start = (i * sent_chunk_size + 1) as u32;
                            let end = (((i + 1) * sent_chunk_size) as u32).min(sent_count);
                            if start > sent_count || sent_count == 0 {
                                return None;
                            }
                            let email_addr = account.email.clone();
                            let password = account.app_password.clone();
                            let counter = Arc::clone(&fetched_count);
                            Some(thread::spawn(move || {
                                retry_silent(|| {
                                    let mut worker_client =
                                        ImapClient::connect(&email_addr, &password)?;
                                    let emails = worker_client.fetch_sent_range(
                                        start,
                                        end,
                                        Some(&counter),
                                    )?;
                                    let _ = worker_client.logout();
                                    Ok::<_, anyhow::Error>(emails)
                                })
                            }))
                        })
                        .collect();

                    // Collect inbox results
                    let mut all_emails = Vec::new();
                    let mut error: Option<anyhow::Error> = None;

                    for handle in inbox_handles {
                        match handle.join() {
                            Ok(Ok(emails)) => all_emails.extend(emails),
                            Ok(Err(e)) => {
                                if error.is_none() {
                                    error = Some(e);
                                }
                            }
                            Err(_) => {
                                if error.is_none() {
                                    error = Some(anyhow::anyhow!("Worker thread panicked"));
                                }
                            }
                        }
                    }

                    // Collect sent results
                    for handle in sent_handles {
                        match handle.join() {
                            Ok(Ok(emails)) => all_emails.extend(emails),
                            Ok(Err(e)) => {
                                if error.is_none() {
                                    error = Some(e);
                                }
                            }
                            Err(_) => {
                                if error.is_none() {
                                    error = Some(anyhow::anyhow!("Worker thread panicked"));
                                }
                            }
                        }
                    }

                    // Signal progress thread to stop and wait for it
                    fetched_count.store(total, Ordering::Relaxed);
                    let _ = progress_handle.join();

                    let result = if let Some(e) = error {
                        debug_log!("FetchInbox: failed with error: {}", e);
                        Err(e)
                    } else {
                        // Dedupe and build thread IDs
                        email::dedupe_emails(&mut all_emails);
                        email::build_thread_ids(&mut all_emails);
                        debug_log!(
                            "FetchInbox: completed in {:.2}s, fetched {} emails",
                            fetch_start.elapsed().as_secs_f64(),
                            all_emails.len()
                        );
                        Ok(all_emails)
                    };

                    let _ = resp_tx.send(ImapResponse::Emails(result));
                }
                ImapCommand::ArchiveMultiple(ids_and_folders) => {
                    use std::collections::HashMap;
                    const BATCH_SIZE: usize = 250;

                    let total = ids_and_folders.len();
                    debug_log!(
                        "ArchiveMultiple: archiving {} emails in batches of {}",
                        total,
                        BATCH_SIZE
                    );
                    let archive_start = Instant::now();

                    // Group by folder
                    let mut by_folder: HashMap<&str, Vec<String>> = HashMap::new();
                    for (uid, folder) in &ids_and_folders {
                        by_folder
                            .entry(folder.as_str())
                            .or_default()
                            .push(uid.clone());
                    }
                    debug_log!(
                        "ArchiveMultiple: grouped into {} source folders",
                        by_folder.len()
                    );

                    let mut all_uid_maps: HashMap<String, u32> = HashMap::new();
                    let mut error: Option<anyhow::Error> = None;
                    let mut processed = 0;

                    'outer: for (folder, uids) in by_folder {
                        debug_log!(
                            "ArchiveMultiple: processing {} emails from '{}'",
                            uids.len(),
                            folder
                        );
                        for (batch_num, chunk) in uids.chunks(BATCH_SIZE).enumerate() {
                            debug_log!(
                                "ArchiveMultiple: batch {} ({} emails, {}/{} total)",
                                batch_num + 1,
                                chunk.len(),
                                processed + chunk.len(),
                                total
                            );
                            let batch_start = Instant::now();

                            let _ = resp_tx.send(ImapResponse::Progress(
                                processed + chunk.len(),
                                total,
                                "Archiving".to_string(),
                            ));
                            let resp_tx_retry = resp_tx.clone();
                            let chunk_result = retry_with_backoff(
                                || client.archive_batch(chunk, folder),
                                |attempt| {
                                    let _ = resp_tx_retry.send(ImapResponse::Retrying {
                                        attempt,
                                        max_attempts: MAX_RETRIES,
                                        action: "archive".to_string(),
                                    });
                                },
                            );
                            debug_log!(
                                "ArchiveMultiple: batch {} completed in {:.3}s",
                                batch_num + 1,
                                batch_start.elapsed().as_secs_f64()
                            );
                            match chunk_result {
                                Ok(uid_map) => {
                                    all_uid_maps.extend(uid_map);
                                    processed += chunk.len();
                                }
                                Err(e) => {
                                    debug_log!("ArchiveMultiple: batch failed: {}", e);
                                    error = Some(e);
                                    break 'outer;
                                }
                            }
                        }
                    }
                    debug_log!(
                        "ArchiveMultiple: finished archiving {} emails in {:.2}s ({})",
                        processed,
                        archive_start.elapsed().as_secs_f64(),
                        if error.is_none() { "success" } else { "failed" }
                    );
                    let result = match error {
                        Some(e) => Err(e),
                        None => Ok(all_uid_maps),
                    };
                    let _ = resp_tx.send(ImapResponse::MultiArchiveResult(result));
                }
                ImapCommand::DeleteMultiple(ids_and_folders) => {
                    use std::collections::HashMap;
                    const BATCH_SIZE: usize = 250;

                    let total = ids_and_folders.len();
                    debug_log!(
                        "DeleteMultiple: deleting {} emails in batches of {}",
                        total,
                        BATCH_SIZE
                    );
                    let delete_start = Instant::now();

                    // Group by folder
                    let mut by_folder: HashMap<&str, Vec<String>> = HashMap::new();
                    for (uid, folder) in &ids_and_folders {
                        by_folder
                            .entry(folder.as_str())
                            .or_default()
                            .push(uid.clone());
                    }
                    debug_log!(
                        "DeleteMultiple: grouped into {} source folders",
                        by_folder.len()
                    );

                    let mut all_uid_maps: HashMap<String, u32> = HashMap::new();
                    let mut error: Option<anyhow::Error> = None;
                    let mut processed = 0;

                    'outer: for (folder, uids) in by_folder {
                        debug_log!(
                            "DeleteMultiple: processing {} emails from '{}'",
                            uids.len(),
                            folder
                        );
                        for (batch_num, chunk) in uids.chunks(BATCH_SIZE).enumerate() {
                            debug_log!(
                                "DeleteMultiple: batch {} ({} emails, {}/{} total)",
                                batch_num + 1,
                                chunk.len(),
                                processed + chunk.len(),
                                total
                            );
                            let batch_start = Instant::now();

                            let _ = resp_tx.send(ImapResponse::Progress(
                                processed + chunk.len(),
                                total,
                                "Deleting".to_string(),
                            ));
                            let resp_tx_retry = resp_tx.clone();
                            let chunk_result = retry_with_backoff(
                                || client.delete_batch(chunk, folder),
                                |attempt| {
                                    let _ = resp_tx_retry.send(ImapResponse::Retrying {
                                        attempt,
                                        max_attempts: MAX_RETRIES,
                                        action: "delete".to_string(),
                                    });
                                },
                            );
                            debug_log!(
                                "DeleteMultiple: batch {} completed in {:.3}s",
                                batch_num + 1,
                                batch_start.elapsed().as_secs_f64()
                            );
                            match chunk_result {
                                Ok(uid_map) => {
                                    all_uid_maps.extend(uid_map);
                                    processed += chunk.len();
                                }
                                Err(e) => {
                                    debug_log!("DeleteMultiple: batch failed: {}", e);
                                    error = Some(e);
                                    break 'outer;
                                }
                            }
                        }
                    }
                    debug_log!(
                        "DeleteMultiple: finished deleting {} emails in {:.2}s ({})",
                        processed,
                        delete_start.elapsed().as_secs_f64(),
                        if error.is_none() { "success" } else { "failed" }
                    );
                    let result = match error {
                        Some(e) => Err(e),
                        None => Ok(all_uid_maps),
                    };
                    let _ = resp_tx.send(ImapResponse::MultiDeleteResult(result));
                }
                ImapCommand::RestoreEmails(restore_ops) => {
                    let total = restore_ops.len();

                    // Create a channel for progress updates
                    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
                    let resp_tx_progress = resp_tx.clone();

                    // Spawn a thread to forward progress updates
                    let progress_thread = std::thread::spawn(move || {
                        let mut processed = 0usize;
                        while let Ok(delta) = progress_rx.recv() {
                            processed += delta;
                            let _ = resp_tx_progress.send(ImapResponse::Progress(
                                processed,
                                total,
                                "Restoring".to_string(),
                            ));
                        }
                    });

                    // Batch restore with retry
                    let resp_tx_retry = resp_tx.clone();
                    let result = retry_with_backoff(
                        || client.restore_emails(&restore_ops, Some(progress_tx.clone())),
                        |attempt| {
                            let _ = resp_tx_retry.send(ImapResponse::Retrying {
                                attempt,
                                max_attempts: MAX_RETRIES,
                                action: "restore".to_string(),
                            });
                        },
                    );

                    // Drop sender to signal completion, then wait for progress thread
                    drop(progress_tx);
                    let _ = progress_thread.join();

                    let _ = resp_tx.send(ImapResponse::RestoreResult(result));
                }
                ImapCommand::FetchBody { uid, folder } => {
                    debug_log!("IMAP worker: fetching body for UID {} from {}", uid, folder);
                    let result = client.fetch_email_body(&uid, &folder);
                    let _ = resp_tx.send(ImapResponse::BodyResult { uid, result });
                }
                ImapCommand::Shutdown => {
                    debug_log!("IMAP worker: shutdown requested");
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
    parallel_connections: usize,
    advance_on_select: bool,
) -> Result<()> {
    let (account_name, account_config) = account;
    let user_email = account_config.email.clone();
    let mut app = App::new();
    app.set_user_email(user_email.clone());
    let mut ui_state = UiState::new();

    // Create channels for IMAP communication
    let (cmd_tx, cmd_rx) = mpsc::channel::<ImapCommand>();
    let (resp_tx, resp_rx) = mpsc::channel::<ImapResponse>();

    // Show connecting status
    ui_state.set_busy(format!("Connecting to {}...", account_name));
    app.ensure_valid_selection();
    terminal.draw(|f| render(f, &app, &mut ui_state))?;

    // Spawn IMAP worker thread
    spawn_imap_worker(cmd_rx, resp_tx, account_config);

    // Wait for connection
    loop {
        // Check for responses
        match resp_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ImapResponse::Connected) => {
                ui_state.set_busy("Loading emails...");
                app.ensure_valid_selection();
                terminal.draw(|f| render(f, &app, &mut ui_state))?;
                cmd_tx.send(ImapCommand::FetchInbox {
                    parallel_connections,
                })?;
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

        app.ensure_valid_selection();
        terminal.draw(|f| render(f, &app, &mut ui_state))?;

        // Check for IMAP responses (non-blocking)
        while let Ok(response) = resp_rx.try_recv() {
            match response {
                ImapResponse::Emails(result) => match result {
                    Ok(emails) => {
                        let email_count = emails.len();
                        app.set_emails(emails);
                        debug_log!(
                            "UI: loaded {} emails into {} groups",
                            email_count,
                            app.groups.len()
                        );
                        ui_state.clear_busy();
                    }
                    Err(e) => {
                        debug_log!("UI: email fetch failed: {}", e);
                        ui_state.clear_busy();
                        ui_state.set_status(format!("Error: {}", e));
                    }
                },
                ImapResponse::MultiArchiveResult(result) => {
                    debug_log!(
                        "UI: multi-archive result: {}",
                        if result.is_ok() { "success" } else { "failed" }
                    );
                    if let Some(op) = pending_operation.take()
                        && let Ok(uid_map) = result
                    {
                        match op {
                            PendingOp::ArchiveGroup { sender, emails } => {
                                // Create undo entry with destination UIDs
                                let undo_emails: Vec<_> = emails
                                    .into_iter()
                                    .map(|(uid, message_id, source_folder)| {
                                        let dest_uid = uid_map.get(&uid).copied();
                                        (message_id, dest_uid, source_folder)
                                    })
                                    .collect();
                                let undo_entry = UndoEntry {
                                    action_type: UndoActionType::Archive,
                                    context: UndoContext::Group { sender },
                                    emails: undo_emails,
                                    current_folder: "[Gmail]/All Mail".to_string(),
                                };
                                app.push_undo(undo_entry);
                                // Remove all emails from threads touched by this group
                                app.remove_current_group_threads();
                            }
                            PendingOp::ArchiveThread {
                                thread_id,
                                subject,
                                emails,
                            } => {
                                // Create undo entry with destination UIDs
                                let undo_emails: Vec<_> = emails
                                    .into_iter()
                                    .map(|(uid, message_id, source_folder)| {
                                        let dest_uid = uid_map.get(&uid).copied();
                                        (message_id, dest_uid, source_folder)
                                    })
                                    .collect();
                                let undo_entry = UndoEntry {
                                    action_type: UndoActionType::Archive,
                                    context: UndoContext::Thread { subject },
                                    emails: undo_emails,
                                    current_folder: "[Gmail]/All Mail".to_string(),
                                };
                                app.push_undo(undo_entry);
                                app.remove_thread(&thread_id);
                                if app.view == View::Thread {
                                    app.exit();
                                }
                            }
                            PendingOp::ArchiveSelected { count, emails } => {
                                // Create undo entry with destination UIDs
                                let undo_emails: Vec<_> = emails
                                    .into_iter()
                                    .map(|(uid, message_id, source_folder)| {
                                        let dest_uid = uid_map.get(&uid).copied();
                                        (message_id, dest_uid, source_folder)
                                    })
                                    .collect();
                                let undo_entry = UndoEntry {
                                    action_type: UndoActionType::Archive,
                                    context: UndoContext::Group {
                                        sender: format!("{} selected", count),
                                    },
                                    emails: undo_emails,
                                    current_folder: "[Gmail]/All Mail".to_string(),
                                };
                                app.push_undo(undo_entry);
                                // Remove all emails from threads touched by selected emails
                                app.remove_selected_threads();
                            }
                            _ => {}
                        }
                    }
                    ui_state.clear_busy();
                }
                ImapResponse::MultiDeleteResult(result) => {
                    debug_log!(
                        "UI: multi-delete result: {}",
                        if result.is_ok() { "success" } else { "failed" }
                    );
                    if let Some(op) = pending_operation.take()
                        && let Ok(uid_map) = result
                    {
                        match op {
                            PendingOp::DeleteGroup { sender, emails } => {
                                // Create undo entry with destination UIDs
                                let undo_emails: Vec<_> = emails
                                    .into_iter()
                                    .map(|(uid, message_id, source_folder)| {
                                        let dest_uid = uid_map.get(&uid).copied();
                                        (message_id, dest_uid, source_folder)
                                    })
                                    .collect();
                                let undo_entry = UndoEntry {
                                    action_type: UndoActionType::Delete,
                                    context: UndoContext::Group { sender },
                                    emails: undo_emails,
                                    current_folder: "[Gmail]/Trash".to_string(),
                                };
                                app.push_undo(undo_entry);
                                // Remove all emails from threads touched by this group
                                app.remove_current_group_threads();
                            }
                            PendingOp::DeleteThread {
                                thread_id,
                                subject,
                                emails,
                            } => {
                                // Create undo entry with destination UIDs
                                let undo_emails: Vec<_> = emails
                                    .into_iter()
                                    .map(|(uid, message_id, source_folder)| {
                                        let dest_uid = uid_map.get(&uid).copied();
                                        (message_id, dest_uid, source_folder)
                                    })
                                    .collect();
                                let undo_entry = UndoEntry {
                                    action_type: UndoActionType::Delete,
                                    context: UndoContext::Thread { subject },
                                    emails: undo_emails,
                                    current_folder: "[Gmail]/Trash".to_string(),
                                };
                                app.push_undo(undo_entry);
                                app.remove_thread(&thread_id);
                                if app.view == View::Thread {
                                    app.exit();
                                }
                            }
                            PendingOp::DeleteSelected { count, emails } => {
                                // Create undo entry with destination UIDs
                                let undo_emails: Vec<_> = emails
                                    .into_iter()
                                    .map(|(uid, message_id, source_folder)| {
                                        let dest_uid = uid_map.get(&uid).copied();
                                        (message_id, dest_uid, source_folder)
                                    })
                                    .collect();
                                let undo_entry = UndoEntry {
                                    action_type: UndoActionType::Delete,
                                    context: UndoContext::Group {
                                        sender: format!("{} selected", count),
                                    },
                                    emails: undo_emails,
                                    current_folder: "[Gmail]/Trash".to_string(),
                                };
                                app.push_undo(undo_entry);
                                // Remove all emails from threads touched by selected emails
                                app.remove_selected_threads();
                            }
                            _ => {}
                        }
                    }
                    ui_state.clear_busy();
                }
                ImapResponse::RestoreResult(result) => {
                    debug_log!(
                        "UI: restore result: {}",
                        if result.is_ok() { "success" } else { "failed" }
                    );
                    if let Some(PendingOp::Undo(index)) = pending_operation.take() {
                        match result {
                            Ok(()) => {
                                debug_log!("UI: undo successful, refreshing emails");
                                // Remove the entry from history
                                app.pop_undo(index);
                                // Stay in undo view - user can close it manually with Escape
                                // Trigger refresh to update the email list
                                ui_state.set_busy("Refreshing...");
                                let _ = cmd_tx.send(ImapCommand::FetchInbox {
                                    parallel_connections,
                                });
                            }
                            Err(e) => {
                                debug_log!("UI: undo failed: {}", e);
                                ui_state.clear_busy();
                                ui_state.set_status(format!("Undo failed: {}", e));
                            }
                        }
                    } else {
                        ui_state.clear_busy();
                    }
                }
                ImapResponse::Progress(current, total, action) => {
                    ui_state.update_busy_message(format!("{} {} of {}...", action, current, total));
                }
                ImapResponse::Retrying {
                    attempt,
                    max_attempts,
                    action,
                } => {
                    ui_state.update_busy_message(format!(
                        "Retrying {} ({}/{})...",
                        action, attempt, max_attempts
                    ));
                }
                ImapResponse::BodyResult { uid, result } => {
                    // Check if we're still viewing this email
                    if app.viewing_email_id() == Some(&uid) {
                        match result {
                            Ok(body) => {
                                // Cache the body and update state
                                app.set_email_body(&uid, body.clone());
                                ui_state.text_view_state = TextViewState::Loaded(body);
                            }
                            Err(e) => {
                                ui_state.text_view_state = TextViewState::Error(format!("{}", e));
                            }
                        }
                    }
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
                            if matches!(action, ConfirmAction::Quit) {
                                let _ = cmd_tx.send(ImapCommand::Shutdown);
                                break;
                            }
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

            // Clear status message on any key press
            if ui_state.has_status() {
                ui_state.clear_status();
                continue; // Consume the key press
            }

            // Handle help menu
            if ui_state.is_showing_help() {
                match key.code {
                    KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                        ui_state.hide_help();
                    }
                    _ => {}
                }
                continue;
            }

            // Handle filter input mode (only in EmailList view)
            if ui_state.is_filter_input_active() {
                match key.code {
                    KeyCode::Esc => {
                        // Clear filter entirely and exit input mode
                        app.clear_text_filter();
                        ui_state.clear_filter_query();
                        ui_state.exit_filter_input_mode();
                    }
                    KeyCode::Enter => {
                        // Exit input mode, keep filter active
                        ui_state.exit_filter_input_mode();
                    }
                    KeyCode::Backspace => {
                        ui_state.backspace_filter();
                        // Update filter in real-time
                        let query = ui_state.filter_query().to_string();
                        if query.is_empty() {
                            app.clear_text_filter();
                        } else {
                            app.set_text_filter(Some(query));
                        }
                    }
                    KeyCode::Char(c) => {
                        ui_state.append_filter_char(c);
                        // Update filter in real-time
                        let query = ui_state.filter_query().to_string();
                        app.set_text_filter(Some(query));
                    }
                    _ => {}
                }
                continue;
            }

            // Toggle help menu with ?
            if key.code == KeyCode::Char('?') {
                ui_state.show_help();
                continue;
            }

            // Enter filter mode with / (only in EmailList view)
            if key.code == KeyCode::Char('/') && app.view == View::EmailList {
                if app.has_text_filter() {
                    // Re-enter input mode with existing query
                    ui_state.enter_filter_input_mode_with_query(app.text_filter().unwrap_or(""));
                } else {
                    // Enter fresh filter input mode
                    ui_state.enter_filter_input_mode();
                }
                continue;
            }

            // Clear pending g for any key that's not part of the gg sequence
            let is_g_sequence = matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G'));
            if !is_g_sequence {
                pending_g = false;
            }

            // Handle UndoHistory view separately
            if app.view == View::UndoHistory {
                match key.code {
                    KeyCode::Char('q') => {
                        ui_state.set_confirm(ConfirmAction::Quit);
                    }
                    KeyCode::Esc => {
                        app.exit_undo_history();
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        app.select_next();
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.select_previous();
                    }
                    KeyCode::Char('g') => {
                        if pending_g {
                            app.select_first();
                            pending_g = false;
                        } else {
                            pending_g = true;
                        }
                    }
                    KeyCode::Char('G') => {
                        pending_g = false;
                        app.select_last();
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let half_page = ui_state.viewport_heights.for_view(app.view) / 2;
                        app.select_next_n(half_page.max(1));
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let half_page = ui_state.viewport_heights.for_view(app.view) / 2;
                        app.select_previous_n(half_page.max(1));
                    }
                    KeyCode::Enter => {
                        // Execute the undo action
                        if let Some(entry) = app.current_undo_entry() {
                            // Build restore ops: (message_id, dest_uid, current_folder, dest_folder)
                            let restore_ops: Vec<(Option<String>, Option<u32>, String, String)> =
                                entry
                                    .emails
                                    .iter()
                                    .map(|(message_id, dest_uid, orig_folder)| {
                                        (
                                            message_id.clone(),
                                            *dest_uid,
                                            entry.current_folder.clone(),
                                            orig_folder.clone(),
                                        )
                                    })
                                    .collect();

                            if !restore_ops.is_empty() {
                                let count = restore_ops.len();
                                ui_state.set_busy(format!("Restoring {} email(s)...", count));
                                pending_operation = Some(PendingOp::Undo(app.selected_undo));
                                let _ = cmd_tx.send(ImapCommand::RestoreEmails(restore_ops));
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }

            // Handle TextView separately
            if app.view == View::EmailBody {
                match key.code {
                    KeyCode::Char('q') => {
                        ui_state.set_confirm(ConfirmAction::Quit);
                    }
                    KeyCode::Esc => {
                        app.exit_text_view();
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        app.scroll_text_view_down(1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.scroll_text_view_up(1);
                    }
                    KeyCode::Char('g') => {
                        if pending_g {
                            app.text_view_scroll = 0;
                            pending_g = false;
                        } else {
                            pending_g = true;
                        }
                    }
                    KeyCode::Char('G') => {
                        pending_g = false;
                        app.text_view_scroll = usize::MAX; // Will be clamped by renderer
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let half_page = ui_state.viewport_heights.text_view / 2;
                        app.scroll_text_view_down(half_page.max(1));
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let half_page = ui_state.viewport_heights.text_view / 2;
                        app.scroll_text_view_up(half_page.max(1));
                    }
                    KeyCode::Char('e') => {
                        // Open in browser
                        if let Some(email) = app.viewing_email() {
                            if let Some(ref message_id) = email.message_id {
                                if let Err(e) = open_email_in_browser(message_id, &user_email) {
                                    ui_state.set_status(format!("Failed to open browser: {}", e));
                                }
                            } else {
                                ui_state.set_status("Email has no Message-ID".to_string());
                            }
                        }
                    }
                    KeyCode::Char('?') => {
                        ui_state.show_help();
                    }
                    _ => {}
                }
                continue;
            }

            // Normal input handling
            match key.code {
                KeyCode::Char('q') => {
                    ui_state.set_confirm(ConfirmAction::Quit);
                }
                KeyCode::Esc => {
                    // In EmailList view with active filter, clear filter instead of exiting
                    if app.view == View::EmailList && app.has_text_filter() {
                        app.clear_text_filter();
                        ui_state.clear_filter_query();
                    } else if app.view != View::GroupList {
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
                        // Enter text view for the selected email in thread
                        if let Some(email) = app.current_thread_email() {
                            let email_id = email.id.clone();
                            let folder = email.source_folder.clone();
                            let has_body = email.body.is_some();
                            app.enter_text_view(&email_id);

                            if has_body {
                                // Body already cached
                                if let Some(body) = app.viewing_email().and_then(|e| e.body.clone())
                                {
                                    ui_state.text_view_state = TextViewState::Loaded(body);
                                }
                            } else {
                                // Need to fetch body
                                ui_state.text_view_state = TextViewState::Loading;
                                cmd_tx.send(ImapCommand::FetchBody {
                                    uid: email_id,
                                    folder,
                                })?;
                            }
                        }
                    } else if app.view == View::EmailList
                        && !app.current_email_is_multi_message_thread()
                    {
                        // Single email - enter text view directly (no thread view needed)
                        if let Some(email) = app.current_email() {
                            let email_id = email.id.clone();
                            let folder = email.source_folder.clone();
                            let has_body = email.body.is_some();
                            app.enter_text_view(&email_id);

                            if has_body {
                                // Body already cached
                                if let Some(body) = app.viewing_email().and_then(|e| e.body.clone())
                                {
                                    ui_state.text_view_state = TextViewState::Loaded(body);
                                }
                            } else {
                                // Need to fetch body
                                ui_state.text_view_state = TextViewState::Loading;
                                cmd_tx.send(ImapCommand::FetchBody {
                                    uid: email_id,
                                    folder,
                                })?;
                            }
                        }
                    } else {
                        app.enter();
                    }
                }
                KeyCode::Char('e') => {
                    // Open email in browser
                    let email_to_open = match app.view {
                        View::Thread => app.current_thread_email(),
                        View::EmailList => app.current_email(),
                        View::EmailBody => app.viewing_email(),
                        _ => None,
                    };
                    if let Some(email) = email_to_open {
                        if let Some(ref message_id) = email.message_id {
                            if let Err(e) = open_email_in_browser(message_id, &user_email) {
                                ui_state.set_status(format!("Failed to open browser: {}", e));
                            }
                        } else {
                            ui_state.set_status("Email has no Message-ID".to_string());
                        }
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
                    if app.view == View::GroupList {
                        app.toggle_group_mode();
                    }
                }
                KeyCode::Char('r') => {
                    ui_state.set_busy("Refreshing...");
                    cmd_tx.send(ImapCommand::FetchInbox {
                        parallel_connections,
                    })?;
                }
                KeyCode::Char('t') => {
                    if app.view == View::GroupList || app.view == View::EmailList {
                        app.toggle_thread_filter();
                    }
                }
                KeyCode::Char('u') => {
                    // Enter undo history view (if history is not empty)
                    app.enter_undo_history();
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
                KeyCode::Char(' ') => {
                    if app.view == View::Thread {
                        ui_state.set_status(
                            "Cannot select individual emails in thread view. Press Enter to open the thread."
                                .to_string(),
                        );
                    } else if app.view == View::EmailList
                        && let app::SelectionResult::Toggled = app.toggle_email_selection()
                        && advance_on_select
                    {
                        app.select_next();
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Tracks pending operations so we know what to update when response arrives
/// Also stores data needed to create undo entries when the result comes back
enum PendingOp {
    /// Archive group: (sender, Vec<(uid, message_id, source_folder)>)
    ArchiveGroup {
        sender: String,
        emails: Vec<(String, Option<String>, String)>,
    },
    /// Delete group: (sender, Vec<(uid, message_id, source_folder)>)
    DeleteGroup {
        sender: String,
        emails: Vec<(String, Option<String>, String)>,
    },
    /// Archive thread: (thread_id, subject, Vec<(uid, message_id, source_folder)>)
    ArchiveThread {
        thread_id: String,
        subject: String,
        emails: Vec<(String, Option<String>, String)>,
    },
    /// Delete thread: (thread_id, subject, Vec<(uid, message_id, source_folder)>)
    DeleteThread {
        thread_id: String,
        subject: String,
        emails: Vec<(String, Option<String>, String)>,
    },
    /// Archive selected: (count_label, Vec<(uid, message_id, source_folder)>)
    ArchiveSelected {
        count: usize,
        emails: Vec<(String, Option<String>, String)>,
    },
    /// Delete selected: (count_label, Vec<(uid, message_id, source_folder)>)
    DeleteSelected {
        count: usize,
        emails: Vec<(String, Option<String>, String)>,
    },
    /// Undo: index in undo history
    Undo(usize),
}

/// Handles the 'a' key - archive thread (not available in thread view)
fn handle_archive(
    app: &mut App,
    cmd_tx: &mpsc::Sender<ImapCommand>,
    ui_state: &mut UiState,
    pending_operation: &mut Option<PendingOp>,
) -> Result<()> {
    match app.view {
        View::GroupList | View::UndoHistory | View::EmailBody => {
            // No action on single 'a' in group list, undo history, or text view
        }
        View::EmailList => {
            // Check if there are selected emails - require confirmation
            if app.has_selection() {
                let count = app.selected_thread_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::ArchiveSelected { count });
                }
                return Ok(());
            }

            // No selection - archive the entire thread
            let email_ids = app.current_thread_email_ids();
            let emails_for_undo = app.current_thread_emails_for_undo();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            let subject = app.current_email().map(|e| e.subject.clone());
            if !email_ids.is_empty()
                && let Some(tid) = thread_id
                && let Some(subj) = subject
            {
                ui_state.set_busy(format!("Archiving thread ({} emails)...", email_ids.len()));
                *pending_operation = Some(PendingOp::ArchiveThread {
                    thread_id: tid,
                    subject: subj,
                    emails: emails_for_undo,
                });
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        View::Thread => {
            // No lowercase 'a' in thread view - use 'A' to archive entire thread
        }
    }
    Ok(())
}

/// Handles the 'A' key - archive all threads in group
fn handle_archive_all(app: &App, ui_state: &mut UiState) {
    match app.view {
        View::GroupList | View::UndoHistory | View::EmailBody => {
            // No 'A' in group list view, undo history, or text view to prevent accidental bulk operations
        }
        View::EmailList => {
            // If there are selected emails, archive those threads
            if app.has_selection() {
                let count = app.selected_thread_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::ArchiveSelected { count });
                }
                return;
            }

            // Archive all threads touched by this group's emails
            if let Some(group) = app.current_group() {
                ui_state.set_confirm(ConfirmAction::ArchiveEmails {
                    sender: group.key.clone(),
                    count: app.current_group_thread_email_ids().len(),
                    filtered: false,
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

/// Handles the 'd' key - delete thread (not available in thread view)
fn handle_delete(
    app: &mut App,
    cmd_tx: &mpsc::Sender<ImapCommand>,
    ui_state: &mut UiState,
    pending_operation: &mut Option<PendingOp>,
) -> Result<()> {
    match app.view {
        View::GroupList | View::UndoHistory | View::EmailBody => {
            // No action on single 'd' in group list, undo history, or text view
        }
        View::EmailList => {
            // Check if there are selected emails - require confirmation
            if app.has_selection() {
                let count = app.selected_thread_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::DeleteSelected { count });
                }
                return Ok(());
            }

            // No selection - delete the entire thread
            let email_ids = app.current_thread_email_ids();
            let emails_for_undo = app.current_thread_emails_for_undo();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            let subject = app.current_email().map(|e| e.subject.clone());
            if !email_ids.is_empty()
                && let Some(tid) = thread_id
                && let Some(subj) = subject
            {
                ui_state.set_busy(format!("Deleting thread ({} emails)...", email_ids.len()));
                *pending_operation = Some(PendingOp::DeleteThread {
                    thread_id: tid,
                    subject: subj,
                    emails: emails_for_undo,
                });
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
        View::Thread => {
            // No lowercase 'd' in thread view - use 'D' to delete entire thread
        }
    }
    Ok(())
}

/// Handles the 'D' key - delete all threads in group
fn handle_delete_all(app: &App, ui_state: &mut UiState) {
    match app.view {
        View::GroupList | View::UndoHistory | View::EmailBody => {
            // No 'D' in group list view, undo history, or text view to prevent accidental bulk operations
        }
        View::EmailList => {
            // If there are selected emails, delete those threads
            if app.has_selection() {
                let count = app.selected_thread_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::DeleteSelected { count });
                }
                return;
            }

            // Delete all threads touched by this group's emails
            if let Some(group) = app.current_group() {
                ui_state.set_confirm(ConfirmAction::DeleteEmails {
                    sender: group.key.clone(),
                    count: app.current_group_thread_email_ids().len(),
                    filtered: false,
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
fn open_email_in_browser(message_id: &str, user_email: &str) -> Result<()> {
    // URL-encode the message ID for safe inclusion in URL
    // Message-IDs typically look like <unique-id@domain.com>
    let encoded = urlencoding::encode(message_id);
    // Use the user's email in the URL path so Gmail opens the correct account
    let url = format!(
        "https://mail.google.com/mail/u/{}/#search/rfc822msgid:{}",
        user_email, encoded
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
    debug_log!("UI: confirmed action: {:?}", action);
    match action {
        ConfirmAction::ArchiveEmails { sender, .. } => {
            // Archive all threads touched by this sender's emails
            let email_ids = app.current_group_thread_email_ids();
            let emails_for_undo = app.current_group_thread_emails_for_undo();
            if !email_ids.is_empty() {
                ui_state.set_busy(format!("Archiving {} emails...", email_ids.len()));
                // Store data for undo entry creation when result arrives
                *pending_operation = Some(PendingOp::ArchiveGroup {
                    sender,
                    emails: emails_for_undo,
                });
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteEmails { sender, .. } => {
            // Delete all threads touched by this sender's emails
            let email_ids = app.current_group_thread_email_ids();
            let emails_for_undo = app.current_group_thread_emails_for_undo();
            if !email_ids.is_empty() {
                ui_state.set_busy(format!("Deleting {} emails...", email_ids.len()));
                // Store data for undo entry creation when result arrives
                *pending_operation = Some(PendingOp::DeleteGroup {
                    sender,
                    emails: emails_for_undo,
                });
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
        ConfirmAction::ArchiveThread { .. } => {
            // Archive entire thread (including user's sent emails)
            let email_ids = app.current_thread_email_ids();
            let emails_for_undo = app.current_thread_emails_for_undo();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            let subject = app.current_email().map(|e| e.subject.clone());
            if !email_ids.is_empty()
                && let Some(tid) = thread_id
                && let Some(subj) = subject
            {
                ui_state.set_busy(format!("Archiving thread ({} emails)...", email_ids.len()));
                // Store data for undo entry creation when result arrives
                *pending_operation = Some(PendingOp::ArchiveThread {
                    thread_id: tid,
                    subject: subj,
                    emails: emails_for_undo,
                });
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteThread { .. } => {
            // Delete entire thread (including user's sent emails)
            let email_ids = app.current_thread_email_ids();
            let emails_for_undo = app.current_thread_emails_for_undo();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            let subject = app.current_email().map(|e| e.subject.clone());
            if !email_ids.is_empty()
                && let Some(tid) = thread_id
                && let Some(subj) = subject
            {
                ui_state.set_busy(format!("Deleting thread ({} emails)...", email_ids.len()));
                // Store data for undo entry creation when result arrives
                *pending_operation = Some(PendingOp::DeleteThread {
                    thread_id: tid,
                    subject: subj,
                    emails: emails_for_undo,
                });
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
        ConfirmAction::ArchiveSelected { count } => {
            // Archive all threads touched by selected emails
            let email_ids = app.selected_thread_email_ids();
            let emails_for_undo = app.selected_thread_emails_for_undo();
            if !email_ids.is_empty() {
                ui_state.set_busy(format!("Archiving {} emails...", email_ids.len()));
                // Store data for undo entry creation when result arrives
                *pending_operation = Some(PendingOp::ArchiveSelected {
                    count,
                    emails: emails_for_undo,
                });
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteSelected { count } => {
            // Delete all threads touched by selected emails
            let email_ids = app.selected_thread_email_ids();
            let emails_for_undo = app.selected_thread_emails_for_undo();
            if !email_ids.is_empty() {
                ui_state.set_busy(format!("Deleting {} emails...", email_ids.len()));
                // Store data for undo entry creation when result arrives
                *pending_operation = Some(PendingOp::DeleteSelected {
                    count,
                    emails: emails_for_undo,
                });
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
        ConfirmAction::Quit => {
            // Handled before calling this function
            unreachable!()
        }
    }
    Ok(())
}
