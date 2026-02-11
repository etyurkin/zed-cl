/// Rust-based LSP server for Common Lisp
///
/// This LSP server is a protocol adapter that forwards requests to the
/// master REPL SBCL process for actual Lisp semantics.

use anyhow::Result;
use tower_lsp::{LspService, Server};
use tracing_subscriber;

mod backend;
mod document;
mod symbol_extractor;
mod symbol_index;
mod user_index;

use backend::LispLspBackend;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging to file - use DEBUG level by default
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/cl-zed-lsp.log")
        .expect("Failed to open log file");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::DEBUG.into()),
        )
        .with_writer(log_file)
        .with_ansi(false)
        .init();

    // Get master REPL ZMQ endpoint from environment or use default
    let endpoint = std::env::var("ZED_CL_MASTER_REPL_ENDPOINT")
        .unwrap_or_else(|_| "tcp://127.0.0.1:5555".to_string());

    // Create LSP backend (paths configured inside backend via env vars)
    let (service, socket) = LspService::new(|client| {
        LispLspBackend::new(client, endpoint)
    });

    // Start LSP server on stdin/stdout
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;

    Ok(())
}
