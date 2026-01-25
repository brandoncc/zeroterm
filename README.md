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
