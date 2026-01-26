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
    let xdg_dirs = xdg::BaseDirectories::with_prefix(APP_NAME)
        .context("Failed to determine config directory")?;
    Ok(xdg_dirs.get_config_home())
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
    credentials_path().map(|p| p.exists()).unwrap_or(false)
}

/// Trait for resolving secrets, allowing for mocking in tests
#[cfg_attr(test, mockall::automock)]
pub trait SecretResolver {
    fn resolve(&self, value: &str) -> Result<String>;
}

/// Real implementation that calls 1Password CLI
pub struct OpSecretResolver;

impl SecretResolver for OpSecretResolver {
    fn resolve(&self, value: &str) -> Result<String> {
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
}

/// Loads credentials from the config file
pub fn load_credentials() -> Result<Credentials> {
    load_credentials_with_resolver(&OpSecretResolver)
}

/// Loads credentials using a provided secret resolver (for testing)
pub fn load_credentials_with_resolver(resolver: &impl SecretResolver) -> Result<Credentials> {
    let path = credentials_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read credentials from {:?}", path))?;
    let credentials: Credentials =
        toml::from_str(&content).context("Failed to parse credentials.toml")?;

    // Resolve app_password if it's a secret reference (e.g., op://vault/item/field)
    let resolved_password = resolver
        .resolve(&credentials.app_password)
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
    fn test_op_resolver_plain_text() {
        let resolver = OpSecretResolver;
        let result = resolver.resolve("plain-password").unwrap();
        assert_eq!(result, "plain-password");
    }

    #[test]
    fn test_op_resolver_returns_value_unchanged_when_not_op_reference() {
        let resolver = OpSecretResolver;
        assert_eq!(resolver.resolve("my-secret").unwrap(), "my-secret");
        assert_eq!(
            resolver.resolve("xxxx xxxx xxxx xxxx").unwrap(),
            "xxxx xxxx xxxx xxxx"
        );
        assert_eq!(resolver.resolve("").unwrap(), "");
        assert_eq!(
            resolver.resolve("op-but-not-reference").unwrap(),
            "op-but-not-reference"
        );
    }

    #[test]
    fn test_mock_resolver_returns_configured_value() {
        let mut mock = MockSecretResolver::new();
        mock.expect_resolve()
            .with(mockall::predicate::eq("op://vault/item/password"))
            .returning(|_| Ok("resolved-secret".to_string()));

        let result = mock.resolve("op://vault/item/password").unwrap();
        assert_eq!(result, "resolved-secret");
    }

    #[test]
    fn test_mock_resolver_can_simulate_error() {
        let mut mock = MockSecretResolver::new();
        mock.expect_resolve()
            .with(mockall::predicate::eq("op://vault/item/password"))
            .returning(|_| Err(anyhow::anyhow!("1Password CLI not found")));

        let result = mock.resolve("op://vault/item/password");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("1Password"));
    }
}
