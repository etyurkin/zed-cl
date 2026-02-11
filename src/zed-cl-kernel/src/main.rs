/// Rust-based Jupyter kernel for Common Lisp
///
/// This kernel is a protocol adapter that handles Jupyter's ZMQ messaging
/// and forwards execution requests to the SBCL master REPL process.

use anyhow::Result;
use std::env;
use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber;

mod kernel;
mod connection;

use kernel::LispKernel;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging to stderr (Jupyter captures stdout)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    info!("Starting Common Lisp Jupyter kernel (Rust)");

    // Get connection file from command line
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        error!("Usage: cl-zed-kernel <connection_file>");
        std::process::exit(1);
    }

    let connection_file = PathBuf::from(&args[1]);
    info!("Using connection file: {:?}", connection_file);

    // Get master REPL ZMQ endpoint
    let master_repl_endpoint = env::var("ZED_CL_MASTER_REPL_ENDPOINT")
        .unwrap_or_else(|_| "tcp://127.0.0.1:5555".to_string());

    info!("Master REPL endpoint: {}", master_repl_endpoint);

    // Create and run kernel
    let kernel = LispKernel::new(connection_file, master_repl_endpoint).await?;
    kernel.run().await?;

    Ok(())
}
