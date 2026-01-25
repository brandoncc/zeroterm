use anyhow::{Context, Result};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use yup_oauth2::{self as oauth2, authenticator::Authenticator};

use crate::config;

/// Creates an OAuth2 authenticator for Gmail API access
pub async fn create_authenticator(
) -> Result<Authenticator<HttpsConnector<HttpConnector>>> {
    let secret_path = config::client_secret_path()?;

    if !secret_path.exists() {
        anyhow::bail!(
            "Client secret file not found at {:?}. \
             Please download OAuth2 credentials from Google Cloud Console \
             and save them as client_secret.json in {:?}",
            secret_path,
            config::config_dir()?
        );
    }

    let secret = oauth2::read_application_secret(&secret_path)
        .await
        .context("Failed to read client secret")?;

    let credentials_path = config::credentials_path()?;

    // Ensure the config directory exists for storing credentials
    config::ensure_config_dir()?;

    let auth = oauth2::InstalledFlowAuthenticator::builder(
        secret,
        oauth2::InstalledFlowReturnMethod::HTTPRedirect,
    )
    .persist_tokens_to_disk(&credentials_path)
    .build()
    .await
    .context("Failed to build authenticator")?;

    Ok(auth)
}

#[cfg(test)]
mod tests {
    // Auth tests require mocking the file system and OAuth flow
    // which is complex - these will be integration tests
}
