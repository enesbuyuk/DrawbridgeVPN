use std::fs;
use std::path::PathBuf;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub vpn_host: String,
    pub vpn_port: String,
    pub email: String,
    pub cert_digest: String,
    #[serde(default)]
    pub use_chrome: bool,
}
impl Profile {
    pub fn new_empty() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            vpn_host: String::new(),
            vpn_port: String::new(),
            email: String::new(),
            cert_digest: String::new(),
            use_chrome: false,
        }
    }
    pub fn saml_url(&self) -> String {
        format!(
            "https://{}:{}/remote/saml/start?realm=",
            self.vpn_host, self.vpn_port
        )
    }
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProfileStore {
    pub profiles: Vec<Profile>,
}
fn profiles_file_path() -> Result<PathBuf> {
    let base = dirs::data_dir().context("Could not determine the Application Support directory")?;
    Ok(base.join("DrawbridgeVPN").join("profiles.json"))
}
impl ProfileStore {
    pub fn load() -> Result<Self> {
        let path = profiles_file_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read profiles file at {}", path.display()))?;
        let store: ProfileStore = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse profiles file at {}", path.display()))?;
        Ok(store)
    }
    pub fn save(&self) -> Result<()> {
        let path = profiles_file_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }
        let data = serde_json::to_string_pretty(self).context("Failed to serialize profiles")?;
        fs::write(&path, data)
            .with_context(|| format!("Failed to write profiles file at {}", path.display()))?;
        Ok(())
    }
    pub fn add(&mut self, profile: Profile) {
        self.profiles.push(profile);
    }
    pub fn update(&mut self, profile: Profile) -> Result<()> {
        let existing = self
            .profiles
            .iter_mut()
            .find(|p| p.id == profile.id)
            .with_context(|| format!("Profile with id {} not found", profile.id))?;
        *existing = profile;
        Ok(())
    }
    pub fn delete(&mut self, id: &str) {
        self.profiles.retain(|p| p.id != id);
    }
    pub fn find(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_saved_before_the_engine_field_still_load() {
        let json = r#"{"profiles":[{
            "id":"abc","name":"Work","vpn_host":"vpn.example.com",
            "vpn_port":"443","email":"a@b.com","cert_digest":"deadbeef"
        }]}"#;
        let store: ProfileStore = serde_json::from_str(json).expect("legacy profile must parse");
        assert_eq!(store.profiles.len(), 1);
        assert!(!store.profiles[0].use_chrome, "legacy profiles default to WKWebView");
    }
    #[test]
    fn new_profiles_default_to_the_webview_engine() {
        assert!(!Profile::new_empty().use_chrome);
    }
    #[test]
    fn the_engine_choice_survives_a_round_trip() {
        let mut profile = Profile::new_empty();
        profile.use_chrome = true;
        let json = serde_json::to_string(&profile).expect("serialise");
        let restored: Profile = serde_json::from_str(&json).expect("deserialise");
        assert!(restored.use_chrome);
    }
}
