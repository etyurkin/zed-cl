/// Symbol indexer for Common Lisp source files
///
/// Indexes Lisp source files into a redb database for fast symbol lookups.
/// This provides reliable goto-definition without depending on SBCL's introspection.

mod extractor;

use anyhow::{Context, Result};
use clap::Parser;
use extractor::SymbolExtractor;
use redb::{Database, ReadableTable, TableDefinition};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Symbol location in a source file
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SymbolLocation {
    /// File ID (reference to files table)
    file_id: u32,
    /// Line number (0-based)
    line: u32,
    /// Character offset in line (0-based)
    character: u32,
    /// Symbol kind: "function", "macro", "variable", etc.
    kind: String,
    /// Package name
    package: String,
}

/// redb table definitions
const FILES_TABLE: TableDefinition<u32, &str> = TableDefinition::new("files");
const SYMBOLS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("symbols");

#[derive(Parser, Debug)]
#[command(name = "zed-cl-index")]
#[command(about = "Index Common Lisp source files for fast symbol lookup")]
struct Args {
    /// Directory to index (recursively searches for .lisp files)
    #[arg(short, long)]
    source_dir: Option<PathBuf>,

    /// Output database file (for index mode)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Default package name for symbols without explicit package
    #[arg(short, long, default_value = "COMMON-LISP-USER")]
    default_package: String,

    /// Query mode: look up a symbol in the database
    #[arg(short, long, num_args = 0)]
    query: bool,

    /// Database file to query (for query mode)
    #[arg(long, requires = "query")]
    database: Option<PathBuf>,

    /// Symbol package (for query mode, optional - searches all packages if omitted)
    #[arg(long, requires = "query")]
    package: Option<String>,

    /// Symbol name (for query mode)
    #[arg(long, requires = "query")]
    symbol: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Query mode
    if args.query {
        let database = args.database.context("--database required in query mode")?;
        let symbol = args.symbol.context("--symbol required in query mode")?;
        return query_database(&database, args.package.as_deref(), &symbol);
    }

    // Index mode
    let source_dir = args.source_dir.context("--source-dir required for indexing")?;
    let output = args.output.context("--output required for indexing")?;

    println!("Indexing Lisp files in: {}", source_dir.display());
    println!("Output database: {}", output.display());

    // Check if database exists before opening
    let db_exists = output.exists();

    // Open existing database or create new one
    let db = if db_exists {
        println!("  Appending to existing database");
        Database::open(&output)
            .with_context(|| format!("Failed to open database at {}", output.display()))?
    } else {
        println!("  Creating new database");
        Database::create(&output)
            .with_context(|| format!("Failed to create database at {}", output.display()))?
    };

    // Start indexing
    let mut file_id = 0u32;
    let mut total_symbols = 0usize;

    {
        let write_txn = db.begin_write()?;
        {
            let mut files_table = write_txn.open_table(FILES_TABLE)?;
            let mut symbols_table = write_txn.open_table(SYMBOLS_TABLE)?;

            // Find the highest file_id if appending
            if db_exists {
                for item in files_table.iter()? {
                    let (id, _) = item?;
                    file_id = file_id.max(id.value() + 1);
                }
            }

            // Walk directory and index .lisp files
            for entry in WalkDir::new(&source_dir)
                .follow_links(true)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("lisp") {
                    match index_file(
                        path,
                        file_id,
                        &args.default_package,
                        &mut files_table,
                        &mut symbols_table,
                    ) {
                        Ok(count) => {
                            println!("  [{}] {} symbols from {}", file_id, count, path.display());
                            total_symbols += count;
                            file_id += 1;
                        }
                        Err(e) => {
                            eprintln!("  Warning: Failed to index {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }
        write_txn.commit()?;
    }

    println!("\nIndexing complete!");
    println!("  Files indexed: {}", file_id);
    println!("  Total symbols: {}", total_symbols);
    println!("  Database size: {} bytes", std::fs::metadata(&output)?.len());

    Ok(())
}

fn index_file(
    path: &Path,
    file_id: u32,
    default_package: &str,
    files_table: &mut redb::Table<u32, &str>,
    symbols_table: &mut redb::Table<&str, &[u8]>,
) -> Result<usize> {
    // Store file path
    let path_str = path.to_str().context("Invalid UTF-8 in path")?;
    files_table.insert(file_id, path_str)?;

    // Read file content
    let source = std::fs::read_to_string(path)?;

    // Extract package from file
    let mut extractor = SymbolExtractor::new()?;
    let package = extractor.extract_package(&source).unwrap_or_else(|| default_package.to_string());

    // Extract all symbol definitions
    let definitions = extractor.extract_definitions(&source);
    let symbol_count = definitions.len();

    // Store each symbol
    for def in definitions {
        let location = SymbolLocation {
            file_id,
            line: def.line,
            character: def.character,
            kind: def.kind,
            package: package.clone(),
        };

        // Create a qualified symbol key: "PACKAGE::SYMBOL"
        let symbol_key = format!("{}::{}", location.package, def.name);

        // Get existing locations for this symbol (if any)
        let mut locations: Vec<SymbolLocation> = symbols_table
            .get(symbol_key.as_str())?
            .map(|bytes: redb::AccessGuard<&[u8]>| {
                bincode::deserialize(bytes.value()).unwrap_or_else(|_| vec![])
            })
            .unwrap_or_else(|| vec![]);

        // Add new location
        locations.push(location);

        // Serialize and store
        let serialized = bincode::serialize(&locations)?;
        symbols_table.insert(symbol_key.as_str(), serialized.as_slice())?;
    }

    Ok(symbol_count)
}

fn query_database(db_path: &Path, package: Option<&str>, symbol: &str) -> Result<()> {
    if !db_path.exists() {
        anyhow::bail!("Database not found: {}", db_path.display());
    }

    let db = Database::open(db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;

    let read_txn = db.begin_read()?;
    let files_table = read_txn.open_table(FILES_TABLE)?;
    let symbols_table = read_txn.open_table(SYMBOLS_TABLE)?;

    let symbol_upper = symbol.to_uppercase();

    // If package specified, search for that specific package::symbol
    if let Some(pkg) = package {
        let symbol_key = format!("{}::{}", pkg.to_uppercase(), symbol_upper);

        println!("Looking up: {}", symbol_key);
        println!();

        match symbols_table.get(symbol_key.as_str())? {
            Some(bytes) => {
                let locations: Vec<SymbolLocation> = bincode::deserialize(bytes.value())
                    .context("Failed to deserialize symbol locations")?;

                println!("Found {} definition(s):", locations.len());
                println!();

                for (i, loc) in locations.iter().enumerate() {
                    let file_path = files_table.get(loc.file_id)?
                        .map(|p| p.value().to_string())
                        .unwrap_or_else(|| format!("<unknown file {}>", loc.file_id));

                    println!("  [{}] {} in {}", i + 1, loc.kind, loc.package);
                    println!("      File: {}", file_path);
                    println!("      Position: line {}, char {}", loc.line, loc.character);
                    println!();
                }

                Ok(())
            }
            None => {
                println!("Symbol not found: {}", symbol_key);
                println!();
                println!("Tip: Omit --package to search all packages, or check package name");
                Ok(())
            }
        }
    } else {
        // No package specified - search all packages for this symbol
        println!("Looking up: {} (in all packages)", symbol_upper);
        println!();

        let mut all_locations = Vec::new();

        // Iterate through all symbols in the database
        for item in symbols_table.iter()? {
            let (key, bytes) = item?;
            let key_str = key.value();

            // Check if this key ends with ::SYMBOL
            if key_str.ends_with(&format!("::{}", symbol_upper)) {
                let locations: Vec<SymbolLocation> = bincode::deserialize(bytes.value())
                    .context("Failed to deserialize symbol locations")?;
                all_locations.extend(locations);
            }
        }

        if all_locations.is_empty() {
            println!("Symbol not found in any package");
            println!();
            println!("Tip: Specify --package to search a specific package");
            Ok(())
        } else {
            println!("Found {} definition(s) across packages:", all_locations.len());
            println!();

            for (i, loc) in all_locations.iter().enumerate() {
                let file_path = files_table.get(loc.file_id)?
                    .map(|p| p.value().to_string())
                    .unwrap_or_else(|| format!("<unknown file {}>", loc.file_id));

                println!("  [{}] {} in {}", i + 1, loc.kind, loc.package);
                println!("      File: {}", file_path);
                println!("      Position: line {}, char {}", loc.line, loc.character);
                println!();
            }

            Ok(())
        }
    }
}
