/// Common Rust library for communicating with the SBCL master REPL
///
/// This library provides a Unix socket client that both the LSP server
/// and Jupyter kernel use to communicate with the master REPL process.

mod config;
mod master_repl;
mod protocol;

pub use config::Config;
pub use master_repl::{MasterReplClient, get_socket_path};
pub use protocol::{DisplayData, ReplRequest, ReplResponse, ResponseData, SymbolInfo};
