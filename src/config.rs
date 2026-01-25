use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_NAME: &str = "zeroterm";
const CREDENTIALS_FILE: &str = "credentials.json";
const CLIENT_SECRET_FILE: &str = "client_secret.json";

/// Configuration for the application
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// The path to the client secret file (OAuth2 credentials from Google)
    pub client_secret_path: Option<PathBuf>,
}

/// Returns the configuration directory path
pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|p| p.join(APP_NAME))
        .context("Failed to determine config directory")
}

/// Returns the path to the credentials file (stored OAuth tokens)
pub fn credentials_path() -> Result<PathBuf> {
    config_dir().map(|p| p.join(CREDENTIALS_FILE))
}

/// Returns the path to the client secret file
pub fn client_secret_path() -> Result<PathBuf> {
    config_dir().map(|p| p.join(CLIENT_SECRET_FILE))
}

/// Ensures the config directory exists
pub fn ensure_config_dir() -> Result<PathBuf> {
    let dir = config_dir()?;
    if !dir.exists() {
        fs::create_dir_all(&dir).context("Failed to create config directory")?;
    }
    Ok(dir)
}

/// Checks if client secret file exists
pub fn has_client_secret() -> bool {
    client_secret_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Checks if credentials (tokens) exist
pub fn has_credentials() -> bool {
    credentials_path()
        .map(|p| p.exists())
        .unwrap_or(false)
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
    fn test_client_secret_path() {
        let path = client_secret_path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.ends_with(CLIENT_SECRET_FILE));
    }
}
