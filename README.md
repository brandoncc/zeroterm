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

- Gmail (via IMAP)

## Installation

```sh
# Clone the repository
git clone https://github.com/brandoncc/zeroterm.git
cd zeroterm

# Build with cargo
cargo build --release

# The binary will be at target/release/zeroterm
```

## Configuration

Zeroterm connects to Gmail via IMAP using an App Password.

### 1. Create a Gmail App Password

1. Go to your [Google Account](https://myaccount.google.com/)
2. Navigate to Security → 2-Step Verification (must be enabled)
3. At the bottom, click "App passwords"
4. Create a new app password for "Mail"
5. Copy the 16-character password

### 2. Create the credentials file

Create `~/.config/zeroterm/credentials.toml`:

```toml
email = "you@gmail.com"
app_password = "xxxx xxxx xxxx xxxx"
```

### Using 1Password CLI (optional)

If you use 1Password, you can reference secrets instead of storing them in plain text:

```toml
email = "you@gmail.com"
app_password = "op://Personal/Gmail App Password/password"
```

Zeroterm will automatically call `op read` to resolve the secret.

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
