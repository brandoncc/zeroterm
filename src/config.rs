use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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

/// Loads credentials from the config file
pub fn load_credentials() -> Result<Credentials> {
    let path = credentials_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read credentials from {:?}", path))?;
    let credentials: Credentials = toml::from_str(&content)
        .context("Failed to parse credentials.toml")?;
    Ok(credentials)
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
}
