/// Query tool for inspecting the symbol index

use anyhow::Result;
use clap::Parser;
use redb::{Database, TableDefinition};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SymbolLocation {
    file_id: u32,
    line: u32,
    character: u32,
    kind: String,
    package: String,
}

const FILES_TABLE: TableDefinition<u32, &str> = TableDefinition::new("files");
const SYMBOLS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("symbols");

#[derive(Parser)]
struct Args {
    /// Database file to query
    #[arg(short, long)]
    database: String,

    /// Symbol to look up (e.g., "COND", "MAPCAR")
    symbol: String,

    /// Package (default: COMMON-LISP)
    #[arg(short, long, default_value = "COMMON-LISP")]
    package: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let db = Database::open(&args.database)?;
    let read_txn = db.begin_read()?;

    let files_table = read_txn.open_table(FILES_TABLE)?;
    let symbols_table = read_txn.open_table(SYMBOLS_TABLE)?;

    // Build symbol key
    let symbol_key = format!("{}::{}", args.package.to_uppercase(), args.symbol.to_uppercase());

    println!("Looking up: {}", symbol_key);

    if let Some(value) = symbols_table.get(symbol_key.as_str())? {
        let locations: Vec<SymbolLocation> = bincode::deserialize(value.value())?;

        println!("\nFound {} location(s):", locations.len());
        for loc in locations {
            let file_path = files_table.get(loc.file_id)?
                .map(|f| f.value().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            println!("\n  File: {}", file_path);
            println!("  Location: line {}, column {}", loc.line, loc.character);
            println!("  Kind: {}", loc.kind);
            println!("  Package: {}", loc.package);
        }
    } else {
        println!("Symbol not found in index");
    }

    Ok(())
}
