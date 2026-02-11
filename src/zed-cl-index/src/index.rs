/// Writable symbol index for incremental updates
///
/// This module provides a writable interface to the symbol index database,
/// allowing files to be indexed, updated, and removed incrementally.

use anyhow::Result;
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::extractor::SymbolExtractor;

// Schema - must match exactly with main indexer
const FILES_TABLE: TableDefinition<u32, &str> = TableDefinition::new("files");
const SYMBOLS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("symbols");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolLocation {
    pub file_id: u32,
    pub line: u32,
    pub character: u32,
    pub kind: String,
    pub package: String,
}

pub struct SymbolIndex {
    db: Database,
    file_id_map: HashMap<PathBuf, u32>,
    next_file_id: u32,
}

impl SymbolIndex {
    /// Open or create a symbol index database
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = Database::create(path.as_ref())?;

        // Load existing file IDs
        let mut file_id_map = HashMap::new();
        let mut next_file_id = 0;

        {
            let read_txn = db.begin_read()?;
            if let Ok(table) = read_txn.open_table(FILES_TABLE) {
                for entry in table.iter()? {
                    let (id, path_str) = entry?;
                    let id_val = id.value();
                    file_id_map.insert(PathBuf::from(path_str.value()), id_val);
                    if id_val >= next_file_id {
                        next_file_id = id_val + 1;
                    }
                }
            }
        }

        Ok(Self {
            db,
            file_id_map,
            next_file_id,
        })
    }

    /// Index a single file, replacing any existing symbols from that file
    pub fn index_file<P: AsRef<Path>>(
        &mut self,
        file_path: P,
        package: &str,
    ) -> Result<usize> {
        let file_path = file_path.as_ref().canonicalize()?;
        let source = std::fs::read_to_string(&file_path)?;

        // Extract symbols
        let mut extractor = SymbolExtractor::new()?;
        let definitions = extractor.extract_definitions(&source);

        // Determine package (use extracted package if available, otherwise use provided)
        let pkg = extractor.extract_package(&source).unwrap_or_else(|| package.to_string());

        // Get or create file ID
        let file_id = if let Some(&id) = self.file_id_map.get(&file_path) {
            id
        } else {
            let id = self.next_file_id;
            self.next_file_id += 1;
            self.file_id_map.insert(file_path.clone(), id);
            id
        };

        // Begin write transaction
        let write_txn = self.db.begin_write()?;

        {
            // Update files table
            let mut files_table = write_txn.open_table(FILES_TABLE)?;
            files_table.insert(file_id, file_path.to_str().unwrap())?;
        }

        {
            // First, remove existing symbols from this file
            let mut symbols_table = write_txn.open_table(SYMBOLS_TABLE)?;

            // Collect keys to remove (can't modify while iterating)
            let mut keys_to_update: Vec<(String, Vec<SymbolLocation>)> = Vec::new();

            for entry in symbols_table.iter()? {
                let (key, value_bytes) = entry?;
                let key_str = key.value().to_string();
                let mut locations: Vec<SymbolLocation> = bincode::deserialize(value_bytes.value())?;

                // Remove locations from this file
                let original_len = locations.len();
                locations.retain(|loc| loc.file_id != file_id);

                if locations.len() != original_len {
                    keys_to_update.push((key_str, locations));
                }
            }

            // Update keys (either remove or update)
            for (key, locations) in keys_to_update {
                if locations.is_empty() {
                    symbols_table.remove(key.as_str())?;
                } else {
                    let serialized = bincode::serialize(&locations)?;
                    symbols_table.insert(key.as_str(), serialized.as_slice())?;
                }
            }

            // Add new symbols
            for def in &definitions {
                let key = format!("{}::{}", pkg.to_uppercase(), def.name.to_uppercase());

                let new_loc = SymbolLocation {
                    file_id,
                    line: def.line,
                    character: def.character,
                    kind: def.kind.clone(),
                    package: pkg.to_uppercase(),
                };

                // Get existing locations or create new vector
                let mut locations = if let Some(value_bytes) = symbols_table.get(key.as_str())? {
                    bincode::deserialize::<Vec<SymbolLocation>>(value_bytes.value())?
                } else {
                    Vec::new()
                };

                // Add new location (we already removed old ones from this file)
                locations.push(new_loc);

                // Store back
                let serialized = bincode::serialize(&locations)?;
                symbols_table.insert(key.as_str(), serialized.as_slice())?;
            }
        }

        write_txn.commit()?;
        Ok(definitions.len())
    }

    /// Remove all symbols from a file
    pub fn remove_file<P: AsRef<Path>>(&mut self, file_path: P) -> Result<()> {
        let file_path = file_path.as_ref().canonicalize()?;

        let file_id = match self.file_id_map.get(&file_path) {
            Some(&id) => id,
            None => return Ok(()), // File not in index
        };

        let write_txn = self.db.begin_write()?;

        {
            // Remove from files table
            let mut files_table = write_txn.open_table(FILES_TABLE)?;
            files_table.remove(file_id)?;
        }

        {
            // Remove from symbols table
            let mut symbols_table = write_txn.open_table(SYMBOLS_TABLE)?;

            let mut keys_to_update: Vec<(String, Vec<SymbolLocation>)> = Vec::new();

            for entry in symbols_table.iter()? {
                let (key, value_bytes) = entry?;
                let key_str = key.value().to_string();
                let mut locations: Vec<SymbolLocation> = bincode::deserialize(value_bytes.value())?;

                let original_len = locations.len();
                locations.retain(|loc| loc.file_id != file_id);

                if locations.len() != original_len {
                    keys_to_update.push((key_str, locations));
                }
            }

            for (key, locations) in keys_to_update {
                if locations.is_empty() {
                    symbols_table.remove(key.as_str())?;
                } else {
                    let serialized = bincode::serialize(&locations)?;
                    symbols_table.insert(key.as_str(), serialized.as_slice())?;
                }
            }
        }

        write_txn.commit()?;
        self.file_id_map.remove(&file_path);
        Ok(())
    }

    /// Look up a symbol in the index
    pub fn lookup(&self, package: &str, symbol: &str) -> Result<Option<(PathBuf, u32, u32)>> {
        let key = format!("{}::{}", package.to_uppercase(), symbol.to_uppercase());

        let read_txn = self.db.begin_read()?;
        let symbols_table = read_txn.open_table(SYMBOLS_TABLE)?;
        let files_table = read_txn.open_table(FILES_TABLE)?;

        if let Some(locations_bytes) = symbols_table.get(key.as_str())? {
            let locations: Vec<SymbolLocation> = bincode::deserialize(locations_bytes.value())?;

            if let Some(first_loc) = locations.first() {
                if let Some(file_path) = files_table.get(first_loc.file_id)? {
                    let path = PathBuf::from(file_path.value());
                    return Ok(Some((path, first_loc.line, first_loc.character)));
                }
            }
        }

        Ok(None)
    }

    /// Get statistics about the index
    pub fn stats(&self) -> Result<IndexStats> {
        let read_txn = self.db.begin_read()?;

        let file_count = {
            let files_table = read_txn.open_table(FILES_TABLE)?;
            files_table.len()?
        };

        let symbol_count = {
            let symbols_table = read_txn.open_table(SYMBOLS_TABLE)?;
            symbols_table.len()?
        };

        Ok(IndexStats {
            file_count: file_count as usize,
            symbol_count: symbol_count as usize,
        })
    }
}

#[derive(Debug)]
pub struct IndexStats {
    pub file_count: usize,
    pub symbol_count: usize,
}
