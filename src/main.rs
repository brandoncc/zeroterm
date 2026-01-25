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
use ui::widgets::{ConfirmAction, UiState};
use ui::render::render;

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
                            match action {
                                ConfirmAction::ArchiveAll { .. } => {
                                    if let Some(group) = app.current_group() {
                                        for email in &group.emails.clone() {
                                            client.archive_email(&email.id).await?;
                                        }
                                    }
                                    app.remove_current_group_emails();
                                }
                                ConfirmAction::DeleteAll { .. } => {
                                    if let Some(group) = app.current_group() {
                                        for email in &group.emails.clone() {
                                            client.delete_email(&email.id).await?;
                                        }
                                    }
                                    app.remove_current_group_emails();
                                }
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

            // Normal input handling
            match key.code {
                KeyCode::Char('q') => {
                    if app.view == View::EmailList {
                        app.exit_group();
                    } else {
                        break;
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => match app.view {
                    View::GroupList => app.select_next_group(),
                    View::EmailList => app.select_next_email(),
                },
                KeyCode::Char('k') | KeyCode::Up => match app.view {
                    View::GroupList => app.select_previous_group(),
                    View::EmailList => app.select_previous_email(),
                },
                KeyCode::Enter => {
                    if app.view == View::GroupList {
                        app.enter_group();
                    }
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
                    if app.view == View::EmailList {
                        if let Some(email) = app.current_email().cloned() {
                            client.archive_email(&email.id).await?;
                            app.remove_email(&email.id);
                        }
                    }
                }
                KeyCode::Char('A') => {
                    if let Some(group) = app.current_group() {
                        ui_state.set_confirm(ConfirmAction::ArchiveAll {
                            sender: group.key.clone(),
                            count: group.count(),
                        });
                    }
                }
                KeyCode::Char('d') => {
                    if app.view == View::EmailList {
                        if let Some(email) = app.current_email().cloned() {
                            client.delete_email(&email.id).await?;
                            app.remove_email(&email.id);
                        }
                    }
                }
                KeyCode::Char('D') => {
                    if let Some(group) = app.current_group() {
                        ui_state.set_confirm(ConfirmAction::DeleteAll {
                            sender: group.key.clone(),
                            count: group.count(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}
