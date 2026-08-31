/// Common library for talking to the master Lisp REPL over TCP.

mod config;
pub mod kernelspec;
mod master_repl;
mod protocol;

pub use config::{data_dir, home_dir, log_dir, Config, Profile, ReplConnection};
pub use master_repl::{connection_file_path, MasterReplClient};
pub use protocol::{parse_lisp_completion_prefix, DisplayData, ReplRequest, ReplResponse, ResponseData, SymbolInfo};
