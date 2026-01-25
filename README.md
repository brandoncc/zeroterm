# Zeroterm

A terminal-based email client designed for achieving inbox zero. Zeroterm groups emails by sender and allows you to quickly archive or delete them—individually or all at once.

## Disclaimer

**This application was 100% vibe coded.** I make no claims about its reliability, safety, or correctness. Use at your own risk. I do not claim responsibility for any outcomes—including but not limited to lost emails, accidentally deleted important messages, or any other data loss—that may result from using this software.

## Features

- **Group by sender email**: Group all emails from a specific address (e.g., `notifications@github.com`)
- **Group by sender domain**: Group all emails from a domain (e.g., `@quora.com`)
- **Individual actions**: Archive or delete specific emails within a group
- **Bulk actions**: Archive or delete all emails in a group at once
- **Keyboard-driven**: Navigate and manage emails entirely via keyboard shortcuts

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `Enter` | Open group / View email |
| `a` | Archive selected email |
| `A` | Archive all emails in group |
| `d` | Delete selected email |
| `D` | Delete all emails in group |
| `g` | Toggle grouping mode (email/domain) |
| `r` | Refresh emails |
| `q` | Quit / Go back |

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
