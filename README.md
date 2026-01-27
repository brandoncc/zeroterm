# Zeroterm

A terminal-based email client designed for achieving inbox zero. Zeroterm groups emails by sender and allows you to quickly archive or delete them—individually or all at once.

## Demo

https://github.com/user-attachments/assets/8d103e57-a7cb-44ff-abef-99f07755d6d2

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
| `gg` | Go to top |
| `G` | Go to bottom |
| `Ctrl+d` | Half page down |
| `Ctrl+u` | Half page up |
| `/` | Search (incremental, like vim) |
| `n` | Next search match |
| `N` | Previous search match |
| `m` | Toggle grouping mode (email/domain) |
| `r` | Refresh emails |
| `q` | Quit / Go back |

### Search Mode
| Key | Action |
|-----|--------|
| Type | Incrementally search and jump to first match |
| `Enter` | Confirm selection and exit search |
| `Escape` | Cancel and restore original selection |
| `Backspace` | Delete last character |

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

Follow the official guide: [Sign in with app passwords](https://support.google.com/accounts/answer/185833)

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

### Thread Protection Mode

By default, Zeroterm requires you to review the full thread before archiving or deleting emails that are part of multi-email threads. Single-email threads can still be archived or deleted from the email list view.

If you prefer the faster workflow without this protection, you can disable it:

```toml
protect_threads = false

[accounts.personal]
backend = "gmail"
email = "you@gmail.com"
app_password = "xxxx xxxx xxxx xxxx"
```

## Usage

```sh
zeroterm
```

### Demo Mode

To try Zeroterm without connecting to an email account, run:

```sh
zeroterm --demo
```

Demo mode loads sample emails and simulates all operations locally. It behaves exactly like the real program, including thread protection warnings and operation feedback, but no actual emails are affected.

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
