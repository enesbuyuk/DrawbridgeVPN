use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};
const SERVICE_NAME: &str = "drawbridgevpn";
#[derive(Serialize, Deserialize, Default)]
struct Credentials {
    password: String,
    totp_secret: String,
}
fn open_entry(profile_id: &str) -> Result<Entry> {
    Entry::new(SERVICE_NAME, profile_id)
        .with_context(|| format!("Failed to open Keychain entry for {profile_id}"))
}
pub fn save_credentials(profile_id: &str, password: &str, totp_secret: &str) -> Result<()> {
    let payload = serde_json::to_string(&Credentials {
        password: password.to_string(),
        totp_secret: totp_secret.to_string(),
    })
    .context("Failed to serialize credentials")?;
    open_entry(profile_id)?
        .set_password(&payload)
        .context("Failed to save the credentials in the Keychain")
}
pub fn load_credentials(profile_id: &str) -> Result<(String, String)> {
    let raw = open_entry(profile_id)?
        .get_password()
        .context("Failed to load the credentials from the Keychain")?;
    let credentials: Credentials =
        serde_json::from_str(&raw).context("Failed to parse stored credentials")?;
    Ok((credentials.password, credentials.totp_secret))
}
pub fn delete_secrets(profile_id: &str) -> Result<()> {
    match open_entry(profile_id)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("Failed to delete the Keychain entry"),
    }
}
