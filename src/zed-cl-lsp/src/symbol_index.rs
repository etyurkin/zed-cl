/// Symbol index integration for goto-definition
///
/// Provides fast lookups of symbol definitions from pre-built redb indexes.
/// Used as first tier before falling back to SBCL introspection.

use anyhow::Result;
use redb::{Database, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::lsp_types::{Location, Position, Range, Url};
use tracing::{debug, error, info};

// Schema from indexer - must match exactly
const SYMBOLS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("symbols");
const FILES_TABLE: TableDefinition<u32, &str> = TableDefinition::new("files");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolLocation {
    pub file_id: u32,
    pub line: u32,
    pub character: u32,
    pub kind: String,
    pub package: String,
}

pub struct SymbolIndex {
    databases: Vec<Database>,
}

impl SymbolIndex {
    /// Create a new symbol index with the given database paths
    pub fn new(index_paths: Vec<PathBuf>) -> Result<Self> {
        let mut databases = Vec::new();

        for path in index_paths {
            if !path.exists() {
                info!("Index file does not exist: {:?}, skipping", path);
                continue;
            }

            match Database::open(&path) {
                Ok(db) => {
                    info!("Loaded symbol index: {:?}", path);
                    databases.push(db);
                }
                Err(e) => {
                    error!("Failed to open index {:?}: {}", path, e);
                }
            }
        }

        Ok(Self { databases })
    }

    /// Look up a symbol in all loaded indexes
    /// Returns the first matching location found
    pub fn lookup(&self, package: &str, symbol: &str) -> Result<Option<Location>> {
        let key = format!("{}::{}", package.to_uppercase(), symbol.to_uppercase());

        for db in &self.databases {
            let read_txn = db.begin_read()?;
            let symbols_table = read_txn.open_table(SYMBOLS_TABLE)?;
            let files_table = read_txn.open_table(FILES_TABLE)?;

            if let Some(locations_bytes) = symbols_table.get(key.as_str())? {
                let locations: Vec<SymbolLocation> =
                    bincode::deserialize(locations_bytes.value())?;

                if let Some(first_loc) = locations.first() {
                    // Look up file path
                    if let Some(file_path) = files_table.get(first_loc.file_id)? {
                        let path = PathBuf::from(file_path.value());
                        if let Ok(uri) = Url::from_file_path(&path) {
                            debug!(
                                "Found {} in index: {}:{}:{}",
                                key, path.display(), first_loc.line, first_loc.character
                            );

                            return Ok(Some(Location {
                                uri,
                                range: Range {
                                    start: Position {
                                        line: first_loc.line,
                                        character: first_loc.character,
                                    },
                                    end: Position {
                                        line: first_loc.line,
                                        character: first_loc.character,
                                    },
                                },
                            }));
                        }
                    }
                }
            }
        }

        debug!("Symbol {} not found in any index", key);
        Ok(None)
    }

    /// Check if any indexes are loaded
    pub fn is_empty(&self) -> bool {
        self.databases.is_empty()
    }
}

/// Thread-safe wrapper for SymbolIndex
pub type SharedSymbolIndex = Arc<RwLock<SymbolIndex>>;
