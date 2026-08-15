/// Jupyter connection file parsing and ZMQ setup

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

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
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read connection file: {:?}", path.as_ref()))?;
        serde_json::from_str(&content).context("Failed to parse connection file")
    }

    pub fn address(&self, port: u16) -> String {
        format!("{}://{}:{}", self.transport, self.ip, port)
    }

    pub fn shell_address(&self) -> String {
        self.address(self.shell_port)
    }

    pub fn iopub_address(&self) -> String {
        self.address(self.iopub_port)
    }

    pub fn control_address(&self) -> String {
        self.address(self.control_port)
    }

    pub fn stdin_address(&self) -> String {
        self.address(self.stdin_port)
    }

    pub fn hb_address(&self) -> String {
        self.address(self.hb_port)
    }
}
