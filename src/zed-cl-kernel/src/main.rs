/// Rust-based Jupyter kernel for Common Lisp

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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    info!("Starting Common Lisp Jupyter kernel (Rust)");

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        error!("Usage: zed-cl-kernel <connection_file>");
        std::process::exit(1);
    }

    let connection_file = PathBuf::from(&args[1]);
    info!("Using connection file: {:?}", connection_file);

    let kernel = LispKernel::new(connection_file).await?;
    kernel.run().await?;

    Ok(())
}
