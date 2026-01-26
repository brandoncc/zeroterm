# Zeroterm Context

Terminal-based Gmail client for achieving inbox zero by grouping emails by sender.

## Architecture

```
src/
├── main.rs       # Entry point, event loop, key handling
├── app.rs        # App state, navigation, grouping logic
├── email.rs      # Email struct, email/domain extraction
├── gmail.rs      # Gmail API wrapper (trait-based for mocking)
├── auth.rs       # OAuth2 authentication
├── config.rs     # XDG config paths
└── ui/
    ├── mod.rs
    ├── render.rs   # Main render function
    └── widgets.rs  # UI components, confirmation dialogs
```

## Key Concepts

### Three-Level Navigation
1. **Group List** - Emails grouped by sender (email or domain)
2. **Email List** - Individual emails from that sender
3. **Thread View** - All emails in a thread (including other senders)

### Thread Handling
- In Group/Email views: actions only affect that sender's emails
- In Thread view: actions affect the entire thread (what you see is what you get)
- Thread view only accepts capital A/D (with confirmation)

### Thread Impact Warnings
- **Email mode**: Shows count of emails from other senders in affected threads
- **Domain mode**: Shows count of threads with multiple participants (other_sender_emails is not meaningful since all domain senders are in the group)

## Constants

- `WARNING_CHAR` in `ui/widgets.rs` - Warning indicator character (⚠)

## Testing

- TDD approach - tests written alongside implementation
- Mock Gmail client via `mockall` crate for API tests
- 43 unit tests covering grouping, navigation, thread handling
