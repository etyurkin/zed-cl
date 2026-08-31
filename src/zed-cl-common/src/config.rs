/// Shared configuration for all zed-cl components.
///
/// Reads from ~/.zed-cl/config.json with profile support.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplConnection {
    pub host: String,
    pub port: u16,
    /// Shared secret the master REPL requires in the connection handshake.
    /// Absent when talking to a pre-1.1 server.
    #[serde(default)]
    pub token: Option<String>,
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
    pub fn get() -> &'static Profile {
        ACTIVE_PROFILE.get_or_init(|| Self::load_active().unwrap_or_default())
    }

    fn load_active() -> Option<Profile> {
        let config_path = data_dir().join("config.json");
        let content = std::fs::read_to_string(config_path).ok()?;
        let config_file = serde_json::from_str::<ConfigFile>(&content).ok()?;
        config_file
            .profiles
            .get(&config_file.active_profile)
            .cloned()
    }

    pub fn connection_file_path(&self) -> PathBuf {
        data_dir().join(format!("repl-{}.json", self.lisp_impl))
    }

    pub fn read_connection(&self) -> Option<ReplConnection> {
        let path = self.connection_file_path();
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }
}

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

pub fn data_dir() -> PathBuf {
    home_dir()
        .map(|h| h.join(".zed-cl"))
        .unwrap_or_else(|| PathBuf::from(".zed-cl"))
}

pub fn log_dir() -> PathBuf {
    let dir = data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub type Config = Profile;
