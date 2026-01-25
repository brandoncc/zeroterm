# Zeroterm

A terminal-based email client designed for achieving inbox zero. Zeroterm groups emails by sender and allows you to quickly archive or delete them—individually or all at once.

## Disclaimer

**This application was vibe coded.** I oversaw the tests being written to make sure they made sense, but I make no claims about its reliability, safety, or correctness. Use at your own risk. I do not claim responsibility for any outcomes—including but not limited to lost emails, accidentally deleted important messages, or any other data loss—that may result from using this software.

## Features

- **Group by sender email**: Group all emails from a specific address (e.g., `notifications@github.com`)
- **Group by sender domain**: Group all emails from a domain (e.g., `@quora.com`)
- **Three-level navigation**: Groups → Emails → Thread view
- **Thread-aware actions**: See exactly what will be affected before archiving/deleting
- **Bulk actions**: Archive or delete all emails from a sender at once
- **Keyboard-driven**: Navigate and manage emails entirely via keyboard shortcuts

## How Thread Handling Works

Zeroterm provides clear control over what gets archived or deleted:

1. **Group/Email views**: Actions only affect emails from that specific sender
   - If a thread contains emails from multiple people, only the selected sender's emails are affected
   - A warning shows when threads contain other participants

2. **Thread view**: Press `Enter` on an email to see the full thread
   - All emails in the thread are shown, including from other senders
   - Actions in this view affect the **entire thread** (what you see is what you get)

This design ensures you always know exactly what emails will be affected before taking action.

## Keyboard Shortcuts

### All Views
| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `g` | Toggle grouping mode (email/domain) |
| `r` | Refresh emails |
| `q` | Quit / Go back |

### Group List View
| Key | Action |
|-----|--------|
| `Enter` | Open group (view emails) |
| `A` | Archive all emails from sender |
| `D` | Delete all emails from sender |

### Email List View
| Key | Action |
|-----|--------|
| `Enter` | View full thread |
| `a` | Archive selected email |
| `A` | Archive all emails from sender |
| `d` | Delete selected email |
| `D` | Delete all emails from sender |

### Thread View
| Key | Action |
|-----|--------|
| `A` | Archive entire thread (with confirmation) |
| `D` | Delete entire thread (with confirmation) |

## Supported Email Providers

- Gmail (via Gmail API)

## Installation

```sh
# Clone the repository
git clone https://github.com/brandoncc/zeroterm.git
cd zeroterm

# Build with cargo
cargo build --release

# The binary will be at target/release/zeroterm
```

## Google Cloud Setup

Before using Zeroterm, you need to set up OAuth2 credentials:

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project (or select an existing one)
3. Enable the Gmail API:
   - Navigate to "APIs & Services" > "Library"
   - Search for "Gmail API" and enable it
4. Configure the OAuth consent screen:
   - Go to "APIs & Services" > "OAuth consent screen"
   - Choose "External" user type
   - Fill in the required fields (app name, user support email, developer contact)
   - Add the scope: `https://mail.google.com/`
   - Add your email as a test user
5. Create OAuth2 credentials:
   - Go to "APIs & Services" > "Credentials"
   - Click "Create Credentials" > "OAuth client ID"
   - Choose "Desktop app" as the application type
   - Download the credentials JSON file
6. Save the credentials file:
   ```sh
   mkdir -p ~/.config/zeroterm
   mv ~/Downloads/client_secret_*.json ~/.config/zeroterm/client_secret.json
   ```

On first run, Zeroterm will open a browser for you to authorize access to your Gmail account.

## Usage

```sh
zeroterm
```

## Development

This project uses [devenv](https://devenv.sh/) for development environment management.

```sh
# Enter the development environment
cd zeroterm
# direnv will automatically load the environment

# Build
cargo build

# Run
cargo run

# Test
cargo test
```

## License

MIT License - see [LICENSE](LICENSE) for details.
