/// Jupyter connection file parsing and ZMQ setup

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Jupyter connection information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub ip: String,
    pub transport: String,
    pub signature_scheme: String,
    pub key: String,

    pub shell_port: u16,
    pub iopub_port: u16,
    pub stdin_port: u16,
    pub control_port: u16,
    pub hb_port: u16,
}

impl ConnectionInfo {
    /// Load connection info from JSON file
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read connection file: {:?}", path.as_ref()))?;

        let info: ConnectionInfo = serde_json::from_str(&content)
            .context("Failed to parse connection file")?;

        Ok(info)
    }

    /// Get full address for a given port
    pub fn address(&self, port: u16) -> String {
        format!("{}://{}:{}", self.transport, self.ip, port)
    }

    /// Shell socket address
    pub fn shell_address(&self) -> String {
        self.address(self.shell_port)
    }

    /// IOPub socket address
    pub fn iopub_address(&self) -> String {
        self.address(self.iopub_port)
    }

    /// Control socket address
    pub fn control_address(&self) -> String {
        self.address(self.control_port)
    }
}
