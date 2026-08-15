/// Common library for talking to the master Lisp REPL over TCP.

mod config;
mod master_repl;
mod protocol;

pub use config::{data_dir, home_dir, log_dir, Config, Profile, ReplConnection};

/// Jupyter kernels subscribe here so they exit when the LSP shuts down.
pub const KERNEL_SHUTDOWN_ENDPOINT: &str = "tcp://127.0.0.1:5557";
pub use master_repl::{connection_file_path, MasterReplClient};
pub use protocol::{DisplayData, ReplRequest, ReplResponse, ResponseData, SymbolInfo};
