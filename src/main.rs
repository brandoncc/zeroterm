mod app;
mod auth;
mod config;
mod email;
mod gmail;
mod ui;

use std::io;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::{App, View};
use auth::create_authenticator;
use gmail::{GmailClient, RealGmailClient};
use ui::render::render;
use ui::widgets::{ConfirmAction, UiState};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize
    config::ensure_config_dir()?;

    // Check for client secret
    if !config::has_client_secret() {
        eprintln!(
            "Error: Client secret not found.\n\
             Please download OAuth2 credentials from Google Cloud Console\n\
             and save them as 'client_secret.json' in {:?}",
            config::config_dir()?
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
    let result = run_app(&mut terminal).await;

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

async fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::new();
    let mut ui_state = UiState::new();

    // Authenticate and create Gmail client
    ui_state.set_status("Authenticating...");
    terminal.draw(|f| render(f, &app, &ui_state))?;

    let auth = create_authenticator().await?;
    let client = RealGmailClient::new(auth).await?;

    // Fetch initial emails
    ui_state.set_status("Loading emails...");
    terminal.draw(|f| render(f, &app, &ui_state))?;

    let emails = client.fetch_inbox().await?;
    app.set_emails(emails);
    ui_state.clear_status();

    // Main event loop
    loop {
        terminal.draw(|f| render(f, &app, &ui_state))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Handle confirmation dialog input
            if ui_state.is_confirming() {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        if let Some(action) = ui_state.confirm_action.take() {
                            handle_confirmed_action(&mut app, &client, action).await?;
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
                    terminal.draw(|f| render(f, &app, &ui_state))?;
                    let emails = client.fetch_inbox().await?;
                    app.set_emails(emails);
                    ui_state.clear_status();
                }
                KeyCode::Char('a') => {
                    handle_archive(&mut app, &client, &mut ui_state).await?;
                }
                KeyCode::Char('A') => {
                    handle_archive_all(&app, &mut ui_state);
                }
                KeyCode::Char('d') => {
                    handle_delete(&mut app, &client, &mut ui_state).await?;
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

/// Handles the 'a' key - archive single email or thread
async fn handle_archive(
    app: &mut App,
    client: &RealGmailClient,
    ui_state: &mut UiState,
) -> Result<()> {
    match app.view {
        View::GroupList => {
            // No action on single 'a' in group list
        }
        View::EmailList => {
            // Archive single email (only this sender's email)
            if let Some(email) = app.current_email().cloned() {
                client.archive_email(&email.id).await?;
                app.remove_email(&email.id);
            }
        }
        View::ThreadView => {
            // In thread view, 'a' archives the entire thread
            let thread_count = app.current_thread_emails().len();
            if thread_count > 0 {
                ui_state.set_confirm(ConfirmAction::ArchiveThread {
                    thread_email_count: thread_count,
                });
            }
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

/// Handles the 'd' key - delete single email or thread
async fn handle_delete(
    app: &mut App,
    client: &RealGmailClient,
    ui_state: &mut UiState,
) -> Result<()> {
    match app.view {
        View::GroupList => {
            // No action on single 'd' in group list
        }
        View::EmailList => {
            // Delete single email (only this sender's email)
            if let Some(email) = app.current_email().cloned() {
                client.delete_email(&email.id).await?;
                app.remove_email(&email.id);
            }
        }
        View::ThreadView => {
            // In thread view, 'd' deletes the entire thread
            let thread_count = app.current_thread_emails().len();
            if thread_count > 0 {
                ui_state.set_confirm(ConfirmAction::DeleteThread {
                    thread_email_count: thread_count,
                });
            }
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
async fn handle_confirmed_action(
    app: &mut App,
    client: &RealGmailClient,
    action: ConfirmAction,
) -> Result<()> {
    match action {
        ConfirmAction::ArchiveEmails { .. } => {
            // Archive only this sender's emails (not full threads)
            let email_ids = app.current_group_email_ids();
            for id in &email_ids {
                client.archive_email(id).await?;
            }
            app.remove_current_group_emails();
        }
        ConfirmAction::DeleteEmails { .. } => {
            // Delete only this sender's emails (not full threads)
            let email_ids = app.current_group_email_ids();
            for id in &email_ids {
                client.delete_email(id).await?;
            }
            app.remove_current_group_emails();
        }
        ConfirmAction::ArchiveThread { .. } => {
            // Archive entire thread
            let email_ids = app.current_thread_email_ids();
            for id in &email_ids {
                client.archive_email(id).await?;
            }
            if let Some(email) = app.current_email() {
                let thread_id = email.thread_id.clone();
                app.remove_thread(&thread_id);
            }
            // Return to email list if we were in thread view
            if app.view == View::ThreadView {
                app.exit();
            }
        }
        ConfirmAction::DeleteThread { .. } => {
            // Delete entire thread
            let email_ids = app.current_thread_email_ids();
            for id in &email_ids {
                client.delete_email(id).await?;
            }
            if let Some(email) = app.current_email() {
                let thread_id = email.thread_id.clone();
                app.remove_thread(&thread_id);
            }
            // Return to email list if we were in thread view
            if app.view == View::ThreadView {
                app.exit();
            }
        }
    }
    Ok(())
}
