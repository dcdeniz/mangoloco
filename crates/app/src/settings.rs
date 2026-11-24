//! User settings, stored as JSON in the app's data directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    /// Speech model id from the catalog, e.g. "base.en".
    pub speech_model: String,
    /// Language code for transcription; empty = detect (multilingual models).
    pub language: String,
    pub transcribe_on_import: bool,
    /// Anthropic API key for the assistant. The ANTHROPIC_API_KEY environment
    /// variable is used when this is empty.
    pub api_key: String,
    pub assistant_model: String,
    pub author: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: true,
            speech_model: "base.en".into(),
            language: String::new(),
            transcribe_on_import: true,
            api_key: String::new(),
            assistant_model: String::new(),
            author: std::env::var("USER").unwrap_or_else(|_| "Me".into()),
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        mango_core::tools::data_dir().join("settings.json")
    }

    /// Loads settings; a missing or unreadable file gives the defaults.
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        // The file can hold an API key: keep it private to the user.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// The API key to use: the saved one, else the environment's.
    pub fn resolved_api_key(&self) -> Option<String> {
        let k = self.api_key.trim();
        if !k.is_empty() {
            return Some(k.to_string());
        }
        std::env::var("ANTHROPIC_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
    }
}
