use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const APP_NAME: &str = "zeroterm";
const CREDENTIALS_FILE: &str = "credentials.toml";

/// IMAP credentials for Gmail access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// Gmail email address
    pub email: String,
    /// Gmail App Password (not regular password)
    pub app_password: String,
}

/// Returns the configuration directory path
pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|p| p.join(APP_NAME))
        .context("Failed to determine config directory")
}

/// Returns the path to the credentials file
pub fn credentials_path() -> Result<PathBuf> {
    config_dir().map(|p| p.join(CREDENTIALS_FILE))
}

/// Ensures the config directory exists
pub fn ensure_config_dir() -> Result<PathBuf> {
    let dir = config_dir()?;
    if !dir.exists() {
        fs::create_dir_all(&dir).context("Failed to create config directory")?;
    }
    Ok(dir)
}

/// Checks if credentials file exists
pub fn has_credentials() -> bool {
    credentials_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Resolves a secret reference (e.g., op://vault/item/field) or returns the value as-is
fn resolve_secret(value: &str) -> Result<String> {
    if value.starts_with("op://") {
        let output = Command::new("op")
            .args(["read", value])
            .output()
            .context("Failed to run 'op' command. Is 1Password CLI installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to read secret from 1Password: {}", stderr.trim());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Ok(value.to_string())
    }
}

/// Loads credentials from the config file
pub fn load_credentials() -> Result<Credentials> {
    let path = credentials_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read credentials from {:?}", path))?;
    let credentials: Credentials = toml::from_str(&content)
        .context("Failed to parse credentials.toml")?;

    // Resolve app_password if it's a secret reference (e.g., op://vault/item/field)
    let resolved_password = resolve_secret(&credentials.app_password)
        .context("Failed to resolve app_password")?;

    Ok(Credentials {
        email: credentials.email,
        app_password: resolved_password,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_dir_not_empty() {
        let dir = config_dir();
        assert!(dir.is_ok());
        let path = dir.unwrap();
        assert!(path.ends_with(APP_NAME));
    }

    #[test]
    fn test_credentials_path() {
        let path = credentials_path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.ends_with(CREDENTIALS_FILE));
    }

    #[test]
    fn test_parse_credentials() {
        let toml_content = r#"
email = "user@gmail.com"
app_password = "xxxx xxxx xxxx xxxx"
"#;
        let credentials: Credentials = toml::from_str(toml_content).unwrap();
        assert_eq!(credentials.email, "user@gmail.com");
        assert_eq!(credentials.app_password, "xxxx xxxx xxxx xxxx");
    }

    #[test]
    fn test_resolve_secret_plain_text() {
        let result = resolve_secret("plain-password").unwrap();
        assert_eq!(result, "plain-password");
    }

    #[test]
    fn test_resolve_secret_returns_value_unchanged_when_not_op_reference() {
        // Various values that should pass through unchanged
        assert_eq!(resolve_secret("my-secret").unwrap(), "my-secret");
        assert_eq!(resolve_secret("xxxx xxxx xxxx xxxx").unwrap(), "xxxx xxxx xxxx xxxx");
        assert_eq!(resolve_secret("").unwrap(), "");
        assert_eq!(resolve_secret("op-but-not-reference").unwrap(), "op-but-not-reference");
    }

    #[test]
    fn test_resolve_secret_detects_op_reference() {
        // We can't actually test 1Password resolution without the CLI,
        // but we can verify the function recognizes op:// prefix
        let result = resolve_secret("op://vault/item/field");
        // This will fail because op CLI isn't available in tests,
        // but that's expected - we're testing the detection logic
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("op") || err.contains("1Password"),
            "Error should mention op or 1Password: {}", err
        );
    }
}
