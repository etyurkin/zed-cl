/// Rust-based LSP server for Common Lisp

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
    let log_path = common_rust::log_dir().join("lsp.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .unwrap_or_else(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("zed-cl-lsp.log")
                .expect("Failed to open log file")
        });

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(log_file)
        .with_ansi(false)
        .init();

    let (service, socket) = LspService::new(|client| LispLspBackend::new(client));

    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;

    Ok(())
}
