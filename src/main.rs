mod app;
mod config;
mod demo;
mod email;
mod imap_client;
mod ui;

use std::io;
use std::process::Command;
use std::sync::mpsc;
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
use ui::widgets::{AccountSelection, ConfirmAction, UiState, WARNING_CHAR};

/// Commands sent to the IMAP worker thread
enum ImapCommand {
    FetchInbox,
    ArchiveEmail(String, String),                 // (uid, folder)
    DeleteEmail(String, String),                  // (uid, folder)
    ArchiveMultiple(Vec<(String, String)>),       // Vec<(uid, folder)>
    DeleteMultiple(Vec<(String, String)>),        // Vec<(uid, folder)>
    RestoreEmails(Vec<(String, String, String)>), // Vec<(uid, current_folder, dest_folder)>
    Shutdown,
}

/// Responses from the IMAP worker thread
enum ImapResponse {
    Emails(Result<Vec<Email>>),
    ArchiveResult(Result<()>),
    DeleteResult(Result<()>),
    MultiArchiveResult(Result<()>),
    MultiDeleteResult(Result<()>),
    RestoreResult(Result<()>),
    /// Progress update during bulk operations (current, total, action)
    Progress(usize, usize, String),
    Connected,
    Error(String),
}

fn main() -> Result<()> {
    // Check for demo mode
    let demo_mode = std::env::args().any(|arg| arg == "--demo");

    if demo_mode {
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
        run_app(&mut terminal, account, cfg.protect_threads)
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
    ArchiveSingle {
        email: Email,
    },
    DeleteSingle {
        email: Email,
    },
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
    },
    DeleteSelected {
        emails: Vec<Email>,
        count: usize,
        processed: usize,
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
            DemoPendingOp::ArchiveSingle { .. }
            | DemoPendingOp::ArchiveGroup { .. }
            | DemoPendingOp::ArchiveThread { .. }
            | DemoPendingOp::ArchiveSelected { .. } => "Archiving...",
            DemoPendingOp::DeleteSingle { .. }
            | DemoPendingOp::DeleteGroup { .. }
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

    // Demo mode uses the default protect_threads setting (true)
    let protect_threads = true;

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
                                        // Record undo entry upfront
                                        let message_ids: Vec<(String, String)> = emails
                                            .iter()
                                            .filter_map(|e| {
                                                e.message_id.as_ref().map(|mid| {
                                                    (mid.clone(), e.source_folder.clone())
                                                })
                                            })
                                            .collect();
                                        if !message_ids.is_empty() {
                                            let undo_entry = UndoEntry {
                                                action_type: UndoActionType::Archive,
                                                context: UndoContext::Group {
                                                    sender: format!("{} selected", count),
                                                },
                                                emails: message_ids,
                                                current_folder: "[Gmail]/All Mail".to_string(),
                                            };
                                            undo_storage.push(emails.clone());
                                            app.push_undo(undo_entry);
                                        }
                                        ui_state.set_busy(format!("Archiving 1 of {}...", count));
                                    }
                                    DemoPendingOp::DeleteSelected { emails, count, .. } => {
                                        // Record undo entry upfront
                                        let message_ids: Vec<(String, String)> = emails
                                            .iter()
                                            .filter_map(|e| {
                                                e.message_id.as_ref().map(|mid| {
                                                    (mid.clone(), e.source_folder.clone())
                                                })
                                            })
                                            .collect();
                                        if !message_ids.is_empty() {
                                            let undo_entry = UndoEntry {
                                                action_type: UndoActionType::Delete,
                                                context: UndoContext::Group {
                                                    sender: format!("{} selected", count),
                                                },
                                                emails: message_ids,
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

            // Handle search mode input
            if ui_state.is_searching() {
                match key.code {
                    KeyCode::Esc => {
                        // Restore original selection and exit
                        if let Some(orig) = ui_state.search_original_selection() {
                            app.restore_selection(
                                orig.selected_group,
                                orig.selected_email,
                                orig.selected_thread_email,
                                orig.selected_undo,
                            );
                        }
                        ui_state.exit_search_mode();
                    }
                    KeyCode::Enter => {
                        // Just exit search mode, keep current selection
                        ui_state.exit_search_mode();
                    }
                    KeyCode::Backspace => {
                        ui_state.backspace_search();
                        // Re-search with updated query
                        let query = ui_state.search_query().to_string();
                        if query.is_empty() {
                            // Restore original selection when query becomes empty
                            if let Some(orig) = ui_state.search_original_selection() {
                                app.restore_selection(
                                    orig.selected_group,
                                    orig.selected_email,
                                    orig.selected_thread_email,
                                    orig.selected_undo,
                                );
                            }
                        } else if !app.search_first(&query) {
                            // No match, restore original selection
                            if let Some(orig) = ui_state.search_original_selection() {
                                app.restore_selection(
                                    orig.selected_group,
                                    orig.selected_email,
                                    orig.selected_thread_email,
                                    orig.selected_undo,
                                );
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        ui_state.append_search_char(c);
                        // Incremental search - find first match
                        let query = ui_state.search_query().to_string();
                        if !app.search_first(&query) {
                            // No match, restore original selection
                            if let Some(orig) = ui_state.search_original_selection() {
                                app.restore_selection(
                                    orig.selected_group,
                                    orig.selected_email,
                                    orig.selected_thread_email,
                                    orig.selected_undo,
                                );
                            }
                        }
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

            // Enter search mode with /
            if key.code == KeyCode::Char('/') {
                ui_state.enter_search_mode(&app);
                continue;
            }

            // Search next/previous with n/N
            if key.code == KeyCode::Char('n') && !ui_state.search_query().is_empty() {
                let query = ui_state.search_query().to_string();
                app.search_next(&query);
                continue;
            }
            if key.code == KeyCode::Char('N') && !ui_state.search_query().is_empty() {
                let query = ui_state.search_query().to_string();
                app.search_previous(&query);
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
                    KeyCode::Char('q') | KeyCode::Esc => {
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

            // Normal input handling
            match key.code {
                KeyCode::Char('q') => {
                    if app.view == View::GroupList {
                        ui_state.set_confirm(ConfirmAction::Quit);
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
                        // In demo mode, open in browser is simulated
                        ui_state.set_status("Demo mode: would open email in browser".to_string());
                    } else if app.view == View::EmailList
                        && !app.current_email_is_multi_message_thread()
                    {
                        // Single email - open directly (no thread view needed)
                        ui_state.set_status("Demo mode: would open email in browser".to_string());
                    } else {
                        app.enter();
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
                    if let Some(op) = handle_demo_archive(&app, &mut ui_state, protect_threads) {
                        ui_state.set_busy(op.busy_message());
                        pending_op = Some(op);
                        op_start_time = Some(Instant::now());
                    }
                }
                KeyCode::Char('A') => {
                    handle_demo_archive_all(&app, &mut ui_state, protect_threads);
                }
                KeyCode::Char('d') => {
                    if let Some(op) = handle_demo_delete(&app, &mut ui_state, protect_threads) {
                        ui_state.set_busy(op.busy_message());
                        pending_op = Some(op);
                        op_start_time = Some(Instant::now());
                    }
                }
                KeyCode::Char('D') => {
                    handle_demo_delete_all(&app, &mut ui_state, protect_threads);
                }
                KeyCode::Char(' ') => {
                    if app.view == View::Thread {
                        ui_state.set_status(
                            "Cannot select individual emails in thread view. Press Enter to open the thread."
                                .to_string(),
                        );
                    } else if app.view == View::EmailList
                        && let app::SelectionResult::IsThread = app.toggle_email_selection()
                    {
                        ui_state.set_status(
                            "Threads must be handled individually. Press Enter to view this thread."
                                .to_string(),
                        );
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
        DemoPendingOp::ArchiveSingle { email } => {
            ui_state.clear_busy();
            if let Some(ref message_id) = email.message_id {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Archive,
                    context: UndoContext::SingleEmail {
                        subject: email.subject.clone(),
                    },
                    emails: vec![(message_id.clone(), email.source_folder.clone())],
                    current_folder: "[Gmail]/All Mail".to_string(),
                };
                undo_storage.push(vec![email.clone()]);
                app.push_undo(undo_entry);
            }
            app.remove_email(&email.id);
            None
        }
        DemoPendingOp::DeleteSingle { email } => {
            ui_state.clear_busy();
            if let Some(ref message_id) = email.message_id {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Delete,
                    context: UndoContext::SingleEmail {
                        subject: email.subject.clone(),
                    },
                    emails: vec![(message_id.clone(), email.source_folder.clone())],
                    current_folder: "[Gmail]/Trash".to_string(),
                };
                undo_storage.push(vec![email.clone()]);
                app.push_undo(undo_entry);
            }
            app.remove_email(&email.id);
            None
        }
        DemoPendingOp::ArchiveGroup { emails, sender } => {
            ui_state.clear_busy();
            let message_ids: Vec<(String, String)> = emails
                .iter()
                .filter_map(|e| {
                    e.message_id
                        .as_ref()
                        .map(|mid| (mid.clone(), e.source_folder.clone()))
                })
                .collect();
            if !message_ids.is_empty() {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Archive,
                    context: UndoContext::Group { sender },
                    emails: message_ids,
                    current_folder: "[Gmail]/All Mail".to_string(),
                };
                undo_storage.push(emails);
                app.push_undo(undo_entry);
            }
            app.remove_current_group_emails();
            None
        }
        DemoPendingOp::DeleteGroup { emails, sender } => {
            ui_state.clear_busy();
            let message_ids: Vec<(String, String)> = emails
                .iter()
                .filter_map(|e| {
                    e.message_id
                        .as_ref()
                        .map(|mid| (mid.clone(), e.source_folder.clone()))
                })
                .collect();
            if !message_ids.is_empty() {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Delete,
                    context: UndoContext::Group { sender },
                    emails: message_ids,
                    current_folder: "[Gmail]/Trash".to_string(),
                };
                undo_storage.push(emails);
                app.push_undo(undo_entry);
            }
            app.remove_current_group_emails();
            None
        }
        DemoPendingOp::ArchiveThread {
            thread_id,
            thread_emails,
            subject,
        } => {
            ui_state.clear_busy();
            let message_ids: Vec<(String, String)> = thread_emails
                .iter()
                .filter_map(|e| {
                    e.message_id
                        .as_ref()
                        .map(|mid| (mid.clone(), e.source_folder.clone()))
                })
                .collect();
            if !message_ids.is_empty() {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Archive,
                    context: UndoContext::Thread { subject },
                    emails: message_ids,
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
            let message_ids: Vec<(String, String)> = thread_emails
                .iter()
                .filter_map(|e| {
                    e.message_id
                        .as_ref()
                        .map(|mid| (mid.clone(), e.source_folder.clone()))
                })
                .collect();
            if !message_ids.is_empty() {
                let undo_entry = UndoEntry {
                    action_type: UndoActionType::Delete,
                    context: UndoContext::Thread { subject },
                    emails: message_ids,
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
        } => {
            // Process one email per cycle for progress display
            if let Some(email) = emails.first().cloned() {
                app.remove_email(&email.id);
                emails.remove(0);
                let new_processed = processed + 1;

                if emails.is_empty() {
                    // All done - record undo entry and clear selection
                    ui_state.clear_busy();
                    // Collect message IDs from all originally selected emails for undo
                    // (we need to get them from the emails we stored, but they're removed now)
                    // For simplicity, we record the undo at the start instead
                    app.clear_selection();
                    None
                } else {
                    // More to process - update progress and continue
                    ui_state.set_busy(format!("Archiving {} of {}...", new_processed + 1, count));
                    Some(DemoPendingOp::ArchiveSelected {
                        emails,
                        count,
                        processed: new_processed,
                    })
                }
            } else {
                ui_state.clear_busy();
                app.clear_selection();
                None
            }
        }
        DemoPendingOp::DeleteSelected {
            mut emails,
            count,
            processed,
        } => {
            // Process one email per cycle for progress display
            if let Some(email) = emails.first().cloned() {
                app.remove_email(&email.id);
                emails.remove(0);
                let new_processed = processed + 1;

                if emails.is_empty() {
                    // All done - clear selection
                    ui_state.clear_busy();
                    app.clear_selection();
                    None
                } else {
                    // More to process - update progress and continue
                    ui_state.set_busy(format!("Deleting {} of {}...", new_processed + 1, count));
                    Some(DemoPendingOp::DeleteSelected {
                        emails,
                        count,
                        processed: new_processed,
                    })
                }
            } else {
                ui_state.clear_busy();
                app.clear_selection();
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
fn handle_demo_archive(
    app: &App,
    ui_state: &mut UiState,
    protect_threads: bool,
) -> Option<DemoPendingOp> {
    match app.view {
        View::GroupList | View::UndoHistory => None,
        View::EmailList => {
            // Check if there are selected emails - require confirmation
            if app.has_selection() {
                let count = app.selected_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::ArchiveSelected { count });
                }
                return None;
            }

            // No selection - archive single email
            // Protect threads: require navigating into thread if it has multiple emails
            let thread_size = app.current_thread_emails().len();
            if protect_threads && thread_size > 1 {
                ui_state.set_status(format!(
                    "{} This thread has {} emails. Press Enter to review the full thread before archiving.",
                    WARNING_CHAR, thread_size
                ));
                return None;
            }
            app.current_email()
                .cloned()
                .map(|email| DemoPendingOp::ArchiveSingle { email })
        }
        View::Thread => None,
    }
}

/// Handles 'A' key in demo mode
fn handle_demo_archive_all(app: &App, ui_state: &mut UiState, protect_threads: bool) {
    match app.view {
        View::GroupList | View::UndoHistory => {}
        View::EmailList => {
            if let Some(group) = app.current_group() {
                // Protect threads: require reviewing each thread separately if group has multi-message threads
                if protect_threads && app.group_has_multi_message_threads(group) {
                    ui_state.set_status(format!(
                        "{} This list contains emails that are part of threads. Each thread must be reviewed and then archived separately.",
                        WARNING_CHAR
                    ));
                    return;
                }
                ui_state.set_confirm(ConfirmAction::ArchiveEmails {
                    sender: group.key.clone(),
                    count: group.count(),
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
fn handle_demo_delete(
    app: &App,
    ui_state: &mut UiState,
    protect_threads: bool,
) -> Option<DemoPendingOp> {
    match app.view {
        View::GroupList | View::UndoHistory => None,
        View::EmailList => {
            // Check if there are selected emails - require confirmation
            if app.has_selection() {
                let count = app.selected_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::DeleteSelected { count });
                }
                return None;
            }

            // No selection - delete single email
            // Protect threads: require navigating into thread if it has multiple emails
            let thread_size = app.current_thread_emails().len();
            if protect_threads && thread_size > 1 {
                ui_state.set_status(format!(
                    "{} This thread has {} emails. Press Enter to review the full thread before deleting.",
                    WARNING_CHAR, thread_size
                ));
                return None;
            }
            app.current_email()
                .cloned()
                .map(|email| DemoPendingOp::DeleteSingle { email })
        }
        View::Thread => None,
    }
}

/// Handles 'D' key in demo mode
fn handle_demo_delete_all(app: &App, ui_state: &mut UiState, protect_threads: bool) {
    match app.view {
        View::GroupList | View::UndoHistory => {}
        View::EmailList => {
            if let Some(group) = app.current_group() {
                // Protect threads: require reviewing each thread separately if group has multi-message threads
                if protect_threads && app.group_has_multi_message_threads(group) {
                    ui_state.set_status(format!(
                        "{} This list contains emails that are part of threads. Each thread must be reviewed and then deleted separately.",
                        WARNING_CHAR
                    ));
                    return;
                }
                ui_state.set_confirm(ConfirmAction::DeleteEmails {
                    sender: group.key.clone(),
                    count: group.count(),
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
            app.current_group()
                .map(|group| DemoPendingOp::ArchiveGroup {
                    emails: group.emails.clone(),
                    sender,
                })
        }
        ConfirmAction::DeleteEmails { sender, .. } => {
            app.current_group().map(|group| DemoPendingOp::DeleteGroup {
                emails: group.emails.clone(),
                sender,
            })
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
            let emails = app.selected_emails_cloned();
            if !emails.is_empty() {
                Some(DemoPendingOp::ArchiveSelected {
                    emails,
                    count,
                    processed: 0,
                })
            } else {
                None
            }
        }
        ConfirmAction::DeleteSelected { count } => {
            let emails = app.selected_emails_cloned();
            if !emails.is_empty() {
                Some(DemoPendingOp::DeleteSelected {
                    emails,
                    count,
                    processed: 0,
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
                    // Fetch from both INBOX and Sent Mail
                    let inbox_result = client.fetch_inbox();
                    let sent_result = client.fetch_sent();

                    let result = match (inbox_result, sent_result) {
                        (Ok(mut inbox), Ok(sent)) => {
                            inbox.extend(sent);
                            // Build thread IDs from combined list
                            email::build_thread_ids(&mut inbox);
                            Ok(inbox)
                        }
                        (Err(e), _) => Err(e),
                        (_, Err(e)) => Err(e),
                    };
                    let _ = resp_tx.send(ImapResponse::Emails(result));
                }
                ImapCommand::ArchiveEmail(id, folder) => {
                    let result = client.archive_email(&id, &folder);
                    let _ = resp_tx.send(ImapResponse::ArchiveResult(result));
                }
                ImapCommand::DeleteEmail(id, folder) => {
                    let result = client.delete_email(&id, &folder);
                    let _ = resp_tx.send(ImapResponse::DeleteResult(result));
                }
                ImapCommand::ArchiveMultiple(ids_and_folders) => {
                    use std::collections::HashMap;
                    const BATCH_SIZE: usize = 250;

                    let total = ids_and_folders.len();

                    // Group by folder
                    let mut by_folder: HashMap<&str, Vec<String>> = HashMap::new();
                    for (uid, folder) in &ids_and_folders {
                        by_folder
                            .entry(folder.as_str())
                            .or_default()
                            .push(uid.clone());
                    }

                    let mut result = Ok(());
                    let mut processed = 0;

                    'outer: for (folder, uids) in by_folder {
                        for chunk in uids.chunks(BATCH_SIZE) {
                            let _ = resp_tx.send(ImapResponse::Progress(
                                processed + chunk.len(),
                                total,
                                "Archiving".to_string(),
                            ));
                            if let Err(e) = client.archive_batch(chunk, folder) {
                                result = Err(e);
                                break 'outer;
                            }
                            processed += chunk.len();
                        }
                    }
                    let _ = resp_tx.send(ImapResponse::MultiArchiveResult(result));
                }
                ImapCommand::DeleteMultiple(ids_and_folders) => {
                    use std::collections::HashMap;
                    const BATCH_SIZE: usize = 250;

                    let total = ids_and_folders.len();

                    // Group by folder
                    let mut by_folder: HashMap<&str, Vec<String>> = HashMap::new();
                    for (uid, folder) in &ids_and_folders {
                        by_folder
                            .entry(folder.as_str())
                            .or_default()
                            .push(uid.clone());
                    }

                    let mut result = Ok(());
                    let mut processed = 0;

                    'outer: for (folder, uids) in by_folder {
                        for chunk in uids.chunks(BATCH_SIZE) {
                            let _ = resp_tx.send(ImapResponse::Progress(
                                processed + chunk.len(),
                                total,
                                "Deleting".to_string(),
                            ));
                            if let Err(e) = client.delete_batch(chunk, folder) {
                                result = Err(e);
                                break 'outer;
                            }
                            processed += chunk.len();
                        }
                    }
                    let _ = resp_tx.send(ImapResponse::MultiDeleteResult(result));
                }
                ImapCommand::RestoreEmails(restore_ops) => {
                    let total = restore_ops.len();
                    let mut result = Ok(());
                    for (i, (message_id, current_folder, dest_folder)) in
                        restore_ops.iter().enumerate()
                    {
                        // Send progress update
                        let _ = resp_tx.send(ImapResponse::Progress(
                            i + 1,
                            total,
                            "Restoring".to_string(),
                        ));
                        // Restore single email
                        let single_restore = [(
                            message_id.clone(),
                            current_folder.clone(),
                            dest_folder.clone(),
                        )];
                        if let Err(e) = client.restore_emails(&single_restore) {
                            result = Err(e);
                            break;
                        }
                    }
                    let _ = resp_tx.send(ImapResponse::RestoreResult(result));
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
    protect_threads: bool,
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

        app.ensure_valid_selection();
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
                            PendingOp::ArchiveSelected => {
                                app.remove_selected_emails();
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
                            PendingOp::DeleteSelected => {
                                app.remove_selected_emails();
                            }
                            _ => {}
                        }
                    }
                    ui_state.clear_busy();
                }
                ImapResponse::RestoreResult(result) => {
                    if let Some(PendingOp::Undo(index)) = pending_operation.take() {
                        match result {
                            Ok(()) => {
                                // Remove the entry from history
                                app.pop_undo(index);
                                // Stay in undo view - user can close it manually with Escape
                                // Trigger refresh to update the email list
                                ui_state.set_busy("Refreshing...");
                                let _ = cmd_tx.send(ImapCommand::FetchInbox);
                            }
                            Err(e) => {
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

            // Handle search mode input
            if ui_state.is_searching() {
                match key.code {
                    KeyCode::Esc => {
                        // Restore original selection and exit
                        if let Some(orig) = ui_state.search_original_selection() {
                            app.restore_selection(
                                orig.selected_group,
                                orig.selected_email,
                                orig.selected_thread_email,
                                orig.selected_undo,
                            );
                        }
                        ui_state.exit_search_mode();
                    }
                    KeyCode::Enter => {
                        // Just exit search mode, keep current selection
                        ui_state.exit_search_mode();
                    }
                    KeyCode::Backspace => {
                        ui_state.backspace_search();
                        // Re-search with updated query
                        let query = ui_state.search_query().to_string();
                        if query.is_empty() {
                            // Restore original selection when query becomes empty
                            if let Some(orig) = ui_state.search_original_selection() {
                                app.restore_selection(
                                    orig.selected_group,
                                    orig.selected_email,
                                    orig.selected_thread_email,
                                    orig.selected_undo,
                                );
                            }
                        } else if !app.search_first(&query) {
                            // No match, restore original selection
                            if let Some(orig) = ui_state.search_original_selection() {
                                app.restore_selection(
                                    orig.selected_group,
                                    orig.selected_email,
                                    orig.selected_thread_email,
                                    orig.selected_undo,
                                );
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        ui_state.append_search_char(c);
                        // Incremental search - find first match
                        let query = ui_state.search_query().to_string();
                        if !app.search_first(&query) {
                            // No match, restore original selection
                            if let Some(orig) = ui_state.search_original_selection() {
                                app.restore_selection(
                                    orig.selected_group,
                                    orig.selected_email,
                                    orig.selected_thread_email,
                                    orig.selected_undo,
                                );
                            }
                        }
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

            // Enter search mode with /
            if key.code == KeyCode::Char('/') {
                ui_state.enter_search_mode(&app);
                continue;
            }

            // Search next/previous with n/N
            if key.code == KeyCode::Char('n') && !ui_state.search_query().is_empty() {
                let query = ui_state.search_query().to_string();
                app.search_next(&query);
                continue;
            }
            if key.code == KeyCode::Char('N') && !ui_state.search_query().is_empty() {
                let query = ui_state.search_query().to_string();
                app.search_previous(&query);
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
                    KeyCode::Char('q') | KeyCode::Esc => {
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
                            let restore_ops: Vec<(String, String, String)> = entry
                                .emails
                                .iter()
                                .map(|(uid, orig_folder)| {
                                    (
                                        uid.clone(),
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

            // Normal input handling
            match key.code {
                KeyCode::Char('q') => {
                    if app.view == View::GroupList {
                        ui_state.set_confirm(ConfirmAction::Quit);
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
                                if let Err(e) = open_email_in_browser(message_id, &user_email) {
                                    ui_state.set_status(format!("Failed to open browser: {}", e));
                                }
                            } else {
                                ui_state.set_status("Email has no Message-ID".to_string());
                            }
                        }
                    } else if app.view == View::EmailList
                        && !app.current_email_is_multi_message_thread()
                    {
                        // Single email - open directly in browser (no thread view needed)
                        if let Some(email) = app.current_email() {
                            if let Some(ref message_id) = email.message_id {
                                if let Err(e) = open_email_in_browser(message_id, &user_email) {
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
                    if app.view == View::GroupList {
                        app.toggle_group_mode();
                    }
                }
                KeyCode::Char('r') => {
                    ui_state.set_busy("Refreshing...");
                    cmd_tx.send(ImapCommand::FetchInbox)?;
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
                    handle_archive(
                        &mut app,
                        &cmd_tx,
                        &mut ui_state,
                        &mut pending_operation,
                        protect_threads,
                    )?;
                }
                KeyCode::Char('A') => {
                    handle_archive_all(&app, &mut ui_state, protect_threads);
                }
                KeyCode::Char('d') => {
                    handle_delete(
                        &mut app,
                        &cmd_tx,
                        &mut ui_state,
                        &mut pending_operation,
                        protect_threads,
                    )?;
                }
                KeyCode::Char('D') => {
                    handle_delete_all(&app, &mut ui_state, protect_threads);
                }
                KeyCode::Char(' ') => {
                    if app.view == View::Thread {
                        ui_state.set_status(
                            "Cannot select individual emails in thread view. Press Enter to open the thread."
                                .to_string(),
                        );
                    } else if app.view == View::EmailList
                        && let app::SelectionResult::IsThread = app.toggle_email_selection()
                    {
                        ui_state.set_status(
                            "Threads must be handled individually. Press Enter to view this thread."
                                .to_string(),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Tracks pending operations so we know what to update when response arrives
enum PendingOp {
    ArchiveSingle(String), // email id
    DeleteSingle(String),  // email id
    ArchiveGroup,
    DeleteGroup,
    ArchiveThread(String), // thread id
    DeleteThread(String),  // thread id
    ArchiveSelected,       // selected emails
    DeleteSelected,        // selected emails
    Undo(usize),           // index in undo history
}

/// Handles the 'a' key - archive single email or selected emails (not available in thread view)
fn handle_archive(
    app: &mut App,
    cmd_tx: &mpsc::Sender<ImapCommand>,
    ui_state: &mut UiState,
    pending_operation: &mut Option<PendingOp>,
    protect_threads: bool,
) -> Result<()> {
    match app.view {
        View::GroupList | View::UndoHistory => {
            // No action on single 'a' in group list or undo history
        }
        View::EmailList => {
            // Check if there are selected emails - require confirmation
            if app.has_selection() {
                let count = app.selected_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::ArchiveSelected { count });
                }
                return Ok(());
            }

            // No selection - archive single email
            // Allow if thread has only one email (nothing else to review)
            let thread_size = app.current_thread_emails().len();
            if protect_threads && thread_size > 1 {
                ui_state.set_status(format!(
                    "{} This thread has {} emails. Press Enter to review the full thread before archiving.",
                    WARNING_CHAR, thread_size
                ));
                return Ok(());
            }
            // Archive single email (only this sender's email)
            if let Some(email) = app.current_email().cloned() {
                // Record undo entry before the action (only if email has Message-ID)
                if let Some(ref message_id) = email.message_id {
                    let undo_entry = UndoEntry {
                        action_type: UndoActionType::Archive,
                        context: UndoContext::SingleEmail {
                            subject: email.subject.clone(),
                        },
                        emails: vec![(message_id.clone(), email.source_folder.clone())],
                        current_folder: "[Gmail]/All Mail".to_string(),
                    };
                    app.push_undo(undo_entry);
                }

                ui_state.set_busy("Archiving...");
                *pending_operation = Some(PendingOp::ArchiveSingle(email.id.clone()));
                cmd_tx.send(ImapCommand::ArchiveEmail(
                    email.id,
                    email.source_folder.clone(),
                ))?;
            }
        }
        View::Thread => {
            // No lowercase 'a' in thread view - use 'A' to archive entire thread
        }
    }
    Ok(())
}

/// Handles the 'A' key - archive all in group
fn handle_archive_all(app: &App, ui_state: &mut UiState, protect_threads: bool) {
    match app.view {
        View::GroupList | View::UndoHistory => {
            // No 'A' in group list view or undo history to prevent accidental bulk operations
        }
        View::EmailList => {
            if let Some(group) = app.current_group() {
                if protect_threads && app.group_has_multi_message_threads(group) {
                    ui_state.set_status(format!(
                        "{} This list contains emails that are part of threads. Each thread must be reviewed and then archived separately.",
                        WARNING_CHAR
                    ));
                    return;
                }
                ui_state.set_confirm(ConfirmAction::ArchiveEmails {
                    sender: group.key.clone(),
                    count: group.count(),
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

/// Handles the 'd' key - delete single email or selected emails (not available in thread view)
fn handle_delete(
    app: &mut App,
    cmd_tx: &mpsc::Sender<ImapCommand>,
    ui_state: &mut UiState,
    pending_operation: &mut Option<PendingOp>,
    protect_threads: bool,
) -> Result<()> {
    match app.view {
        View::GroupList | View::UndoHistory => {
            // No action on single 'd' in group list or undo history
        }
        View::EmailList => {
            // Check if there are selected emails - require confirmation
            if app.has_selection() {
                let count = app.selected_email_ids().len();
                if count > 0 {
                    ui_state.set_confirm(ConfirmAction::DeleteSelected { count });
                }
                return Ok(());
            }

            // No selection - delete single email
            // Allow if thread has only one email (nothing else to review)
            let thread_size = app.current_thread_emails().len();
            if protect_threads && thread_size > 1 {
                ui_state.set_status(format!(
                    "{} This thread has {} emails. Press Enter to review the full thread before deleting.",
                    WARNING_CHAR, thread_size
                ));
                return Ok(());
            }
            // Delete single email (only this sender's email)
            if let Some(email) = app.current_email().cloned() {
                // Record undo entry before the action (only if email has Message-ID)
                if let Some(ref message_id) = email.message_id {
                    let undo_entry = UndoEntry {
                        action_type: UndoActionType::Delete,
                        context: UndoContext::SingleEmail {
                            subject: email.subject.clone(),
                        },
                        emails: vec![(message_id.clone(), email.source_folder.clone())],
                        current_folder: "[Gmail]/Trash".to_string(),
                    };
                    app.push_undo(undo_entry);
                }

                ui_state.set_busy("Deleting...");
                *pending_operation = Some(PendingOp::DeleteSingle(email.id.clone()));
                cmd_tx.send(ImapCommand::DeleteEmail(
                    email.id,
                    email.source_folder.clone(),
                ))?;
            }
        }
        View::Thread => {
            // No lowercase 'd' in thread view - use 'D' to delete entire thread
        }
    }
    Ok(())
}

/// Handles the 'D' key - delete all in group
fn handle_delete_all(app: &App, ui_state: &mut UiState, protect_threads: bool) {
    match app.view {
        View::GroupList | View::UndoHistory => {
            // No 'D' in group list view or undo history to prevent accidental bulk operations
        }
        View::EmailList => {
            if let Some(group) = app.current_group() {
                if protect_threads && app.group_has_multi_message_threads(group) {
                    ui_state.set_status(format!(
                        "{} This list contains emails that are part of threads. Each thread must be reviewed and then deleted separately.",
                        WARNING_CHAR
                    ));
                    return;
                }
                ui_state.set_confirm(ConfirmAction::DeleteEmails {
                    sender: group.key.clone(),
                    count: group.count(),
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
    match action {
        ConfirmAction::ArchiveEmails { sender, .. } => {
            // Archive only this sender's emails (not full threads)
            let email_ids = app.current_group_email_ids();
            let message_ids = app.current_group_message_ids();
            if !email_ids.is_empty() {
                // Record undo entry before the action (using Message-IDs for restore)
                if !message_ids.is_empty() {
                    let undo_entry = UndoEntry {
                        action_type: UndoActionType::Archive,
                        context: UndoContext::Group {
                            sender: sender.clone(),
                        },
                        emails: message_ids,
                        current_folder: "[Gmail]/All Mail".to_string(),
                    };
                    app.push_undo(undo_entry);
                }

                ui_state.set_busy(format!("Archiving {} emails...", email_ids.len()));
                *pending_operation = Some(PendingOp::ArchiveGroup);
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteEmails { sender, .. } => {
            // Delete only this sender's emails (not full threads)
            let email_ids = app.current_group_email_ids();
            let message_ids = app.current_group_message_ids();
            if !email_ids.is_empty() {
                // Record undo entry before the action (using Message-IDs for restore)
                if !message_ids.is_empty() {
                    let undo_entry = UndoEntry {
                        action_type: UndoActionType::Delete,
                        context: UndoContext::Group {
                            sender: sender.clone(),
                        },
                        emails: message_ids,
                        current_folder: "[Gmail]/Trash".to_string(),
                    };
                    app.push_undo(undo_entry);
                }

                ui_state.set_busy(format!("Deleting {} emails...", email_ids.len()));
                *pending_operation = Some(PendingOp::DeleteGroup);
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
        ConfirmAction::ArchiveThread { .. } => {
            // Archive entire thread (including user's sent emails)
            let email_ids = app.current_thread_email_ids();
            let message_ids = app.current_thread_message_ids();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            let subject = app.current_email().map(|e| e.subject.clone());
            if !email_ids.is_empty()
                && let Some(tid) = thread_id
                && let Some(subj) = subject
            {
                // Record undo entry before the action (using Message-IDs for restore)
                if !message_ids.is_empty() {
                    let undo_entry = UndoEntry {
                        action_type: UndoActionType::Archive,
                        context: UndoContext::Thread {
                            subject: subj.clone(),
                        },
                        emails: message_ids,
                        current_folder: "[Gmail]/All Mail".to_string(),
                    };
                    app.push_undo(undo_entry);
                }

                ui_state.set_busy(format!("Archiving thread ({} emails)...", email_ids.len()));
                *pending_operation = Some(PendingOp::ArchiveThread(tid));
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteThread { .. } => {
            // Delete entire thread (including user's sent emails)
            let email_ids = app.current_thread_email_ids();
            let message_ids = app.current_thread_message_ids();
            let thread_id = app.current_email().map(|e| e.thread_id.clone());
            let subject = app.current_email().map(|e| e.subject.clone());
            if !email_ids.is_empty()
                && let Some(tid) = thread_id
                && let Some(subj) = subject
            {
                // Record undo entry before the action (using Message-IDs for restore)
                if !message_ids.is_empty() {
                    let undo_entry = UndoEntry {
                        action_type: UndoActionType::Delete,
                        context: UndoContext::Thread {
                            subject: subj.clone(),
                        },
                        emails: message_ids,
                        current_folder: "[Gmail]/Trash".to_string(),
                    };
                    app.push_undo(undo_entry);
                }

                ui_state.set_busy(format!("Deleting thread ({} emails)...", email_ids.len()));
                *pending_operation = Some(PendingOp::DeleteThread(tid));
                cmd_tx.send(ImapCommand::DeleteMultiple(email_ids))?;
            }
        }
        ConfirmAction::ArchiveSelected { count } => {
            let email_ids = app.selected_email_ids();
            let message_ids = app.selected_message_ids();
            if !email_ids.is_empty() {
                // Record undo entry before the action
                if !message_ids.is_empty() {
                    let undo_entry = UndoEntry {
                        action_type: UndoActionType::Archive,
                        context: UndoContext::Group {
                            sender: format!("{} selected", count),
                        },
                        emails: message_ids,
                        current_folder: "[Gmail]/All Mail".to_string(),
                    };
                    app.push_undo(undo_entry);
                }

                ui_state.set_busy(format!("Archiving {} emails...", email_ids.len()));
                *pending_operation = Some(PendingOp::ArchiveSelected);
                cmd_tx.send(ImapCommand::ArchiveMultiple(email_ids))?;
            }
        }
        ConfirmAction::DeleteSelected { count } => {
            let email_ids = app.selected_email_ids();
            let message_ids = app.selected_message_ids();
            if !email_ids.is_empty() {
                // Record undo entry before the action
                if !message_ids.is_empty() {
                    let undo_entry = UndoEntry {
                        action_type: UndoActionType::Delete,
                        context: UndoContext::Group {
                            sender: format!("{} selected", count),
                        },
                        emails: message_ids,
                        current_folder: "[Gmail]/Trash".to_string(),
                    };
                    app.push_undo(undo_entry);
                }

                ui_state.set_busy(format!("Deleting {} emails...", email_ids.len()));
                *pending_operation = Some(PendingOp::DeleteSelected);
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
