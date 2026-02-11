/// Shared configuration for all zed-cl components
///
/// Reads from ~/.zed-cl/config.json with profile support

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

static ACTIVE_PROFILE: OnceLock<Profile> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default = "default_active_profile")]
    pub active_profile: String,

    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default = "default_lisp_impl")]
    pub lisp_impl: String,

    #[serde(default = "default_system_index")]
    pub system_index: String,

    #[serde(default)]
    pub completion_package_whitelist: Option<Vec<String>>,
}

fn default_active_profile() -> String {
    "sbcl".to_string()
}

fn default_lisp_impl() -> String {
    "sbcl".to_string()
}

fn default_system_index() -> String {
    "system-index.db".to_string()
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            lisp_impl: default_lisp_impl(),
            system_index: default_system_index(),
            completion_package_whitelist: None,
        }
    }
}

impl Profile {
    /// Get the active profile instance
    pub fn get() -> &'static Profile {
        ACTIVE_PROFILE.get_or_init(|| {
            Self::load_active().unwrap_or_default()
        })
    }

    /// Load the active profile from ~/.zed-cl/config.json
    fn load_active() -> Option<Profile> {
        let home = std::env::var("HOME").ok()?;
        let config_path = PathBuf::from(home).join(".zed-cl/config.json");

        let content = std::fs::read_to_string(config_path).ok()?;
        let config_file = serde_json::from_str::<ConfigFile>(&content).ok()?;

        config_file.profiles.get(&config_file.active_profile).cloned()
    }

    /// Get implementation-specific socket path (dynamic property)
    pub fn socket_path(&self) -> PathBuf {
        PathBuf::from(format!("/tmp/zed-cl-repl-{}.sock", self.lisp_impl))
    }
}

// Maintain backward compatibility with old Config name
pub type Config = Profile;
