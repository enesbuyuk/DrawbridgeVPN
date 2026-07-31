pub mod chrome;
pub mod script;
pub mod webview;
pub use chrome::cleanup_browsers;
use std::sync::mpsc::Sender;
use anyhow::{anyhow, Result};
use totp_rs::{Algorithm, Secret, TOTP};
use crate::events::LogEvent;
use crate::profile::Profile;
fn normalize_base32_secret(secret: &str) -> String {
    let mut cleaned: String = secret
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let padding_needed = (8 - cleaned.len() % 8) % 8;
    cleaned.extend(std::iter::repeat('=').take(padding_needed));
    cleaned
}
fn build_totp(secret: &str) -> Result<TOTP> {
    let normalized = normalize_base32_secret(secret);
    let secret_bytes = Secret::Encoded(normalized)
        .to_bytes()
        .map_err(|e| anyhow!("Invalid TOTP secret: {e:?}"))?;
    Ok(TOTP::new_unchecked(Algorithm::SHA1, 6, 1, 30, secret_bytes))
}
pub async fn login_and_get_cookie(
    profile: &Profile,
    password: &str,
    totp_secret: &str,
    log_tx: Sender<LogEvent>,
) -> Result<String> {
    let totp = build_totp(totp_secret)?;
    if profile.use_chrome {
        chrome::login_and_get_cookie(profile, password, &totp, log_tx).await
    } else {
        webview::login_and_get_cookie(profile, password, &totp, log_tx).await
    }
}
