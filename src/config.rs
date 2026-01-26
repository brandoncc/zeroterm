use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const APP_NAME: &str = "zeroterm";
const CONFIG_FILE: &str = "config.toml";

/// Supported email backends
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Gmail,
}

/// Configuration for a single email account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// The backend type for this account
    pub backend: Backend,
    /// Email address
    pub email: String,
    /// App Password (not regular password)
    pub app_password: String,
}

/// Top-level configuration containing all accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Named accounts, keyed by account name
    pub accounts: HashMap<String, AccountConfig>,
    /// When true, archive/delete only work in thread view
    #[serde(default)]
    pub protect_threads: bool,
}

/// Returns the configuration directory path
pub fn config_dir() -> Result<PathBuf> {
    let xdg_dirs = xdg::BaseDirectories::with_prefix(APP_NAME)
        .context("Failed to determine config directory")?;
    Ok(xdg_dirs.get_config_home())
}

/// Returns the path to the config file
pub fn config_path() -> Result<PathBuf> {
    config_dir().map(|p| p.join(CONFIG_FILE))
}

/// Ensures the config directory exists
pub fn ensure_config_dir() -> Result<PathBuf> {
    let dir = config_dir()?;
    if !dir.exists() {
        fs::create_dir_all(&dir).context("Failed to create config directory")?;
    }
    Ok(dir)
}

/// Checks if config file exists
pub fn has_config() -> bool {
    config_path().map(|p| p.exists()).unwrap_or(false)
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

/// Loads config from the config file
pub fn load_config() -> Result<Config> {
    load_config_with_resolver(&OpSecretResolver)
}

/// Loads config using a provided secret resolver (for testing)
pub fn load_config_with_resolver(resolver: &impl SecretResolver) -> Result<Config> {
    let path = config_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config from {:?}", path))?;
    let config: Config = toml::from_str(&content).context("Failed to parse config.toml")?;

    if config.accounts.is_empty() {
        anyhow::bail!("No accounts configured in config.toml");
    }

    // Resolve app_password for each account
    let mut resolved_accounts = HashMap::new();
    for (name, account) in config.accounts {
        let resolved_password = resolver
            .resolve(&account.app_password)
            .with_context(|| format!("Failed to resolve app_password for account '{}'", name))?;

        resolved_accounts.insert(
            name,
            AccountConfig {
                backend: account.backend,
                email: account.email,
                app_password: resolved_password,
            },
        );
    }

    Ok(Config {
        accounts: resolved_accounts,
        protect_threads: config.protect_threads,
    })
}

/// Gets the first account from the config (useful when only one account exists)
pub fn get_default_account(config: &Config) -> Result<(&String, &AccountConfig)> {
    config
        .accounts
        .iter()
        .min_by_key(|(name, _)| name.as_str())
        .context("No accounts configured")
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
    fn test_config_path() {
        let path = config_path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.ends_with(CONFIG_FILE));
    }

    #[test]
    fn test_parse_single_account_config() {
        let toml_content = r#"
[accounts.personal]
backend = "gmail"
email = "user@gmail.com"
app_password = "xxxx xxxx xxxx xxxx"
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert_eq!(config.accounts.len(), 1);
        assert!(!config.protect_threads);
        let account = config.accounts.get("personal").unwrap();
        assert_eq!(account.backend, Backend::Gmail);
        assert_eq!(account.email, "user@gmail.com");
        assert_eq!(account.app_password, "xxxx xxxx xxxx xxxx");
    }

    #[test]
    fn test_protect_threads_defaults_to_false() {
        let toml_content = r#"
[accounts.personal]
backend = "gmail"
email = "user@gmail.com"
app_password = "xxxx"
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(!config.protect_threads);
    }

    #[test]
    fn test_protect_threads_can_be_enabled() {
        let toml_content = r#"
protect_threads = true

[accounts.personal]
backend = "gmail"
email = "user@gmail.com"
app_password = "xxxx"
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(config.protect_threads);
    }

    #[test]
    fn test_parse_multiple_accounts_config() {
        let toml_content = r#"
[accounts.personal]
backend = "gmail"
email = "personal@gmail.com"
app_password = "xxxx xxxx xxxx xxxx"

[accounts.work]
backend = "gmail"
email = "work@company.com"
app_password = "yyyy yyyy yyyy yyyy"
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert_eq!(config.accounts.len(), 2);

        let personal = config.accounts.get("personal").unwrap();
        assert_eq!(personal.backend, Backend::Gmail);
        assert_eq!(personal.email, "personal@gmail.com");

        let work = config.accounts.get("work").unwrap();
        assert_eq!(work.backend, Backend::Gmail);
        assert_eq!(work.email, "work@company.com");
    }

    #[test]
    fn test_backend_requires_valid_value() {
        let toml_content = r#"
[accounts.test]
backend = "outlook"
email = "user@outlook.com"
app_password = "xxxx"
"#;
        let result: Result<Config, _> = toml::from_str(toml_content);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_default_account() {
        let toml_content = r#"
[accounts.personal]
backend = "gmail"
email = "user@gmail.com"
app_password = "xxxx"
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        let (name, account) = get_default_account(&config).unwrap();
        assert_eq!(name, "personal");
        assert_eq!(account.email, "user@gmail.com");
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
