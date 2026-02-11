/// User file indexing for incremental symbol tracking
///
/// Automatically indexes user files on open/save to provide fast goto-definition
/// without requiring SBCL evaluation.

use anyhow::Result;
use std::path::PathBuf;
use tower_lsp::lsp_types::Url;
use tracing::{debug, error, info};
use zed_cl_indexer::SymbolIndex;

pub struct UserIndexManager {
    index: Option<SymbolIndex>,
    index_path: PathBuf,
}

impl UserIndexManager {
    /// Create a new user index manager
    pub fn new(index_path: PathBuf) -> Self {
        // Try to open or create the index
        let index = match SymbolIndex::open(&index_path) {
            Ok(idx) => {
                info!("User index opened: {:?}", index_path);
                Some(idx)
            }
            Err(e) => {
                error!("Failed to open user index {:?}: {}", index_path, e);
                None
            }
        };

        Self { index, index_path }
    }

    /// Index a file (called on didOpen/didSave)
    pub fn index_file(&mut self, uri: &Url, package: &str) -> Result<()> {
        let path = uri
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("Invalid file path"))?;

        if let Some(ref mut index) = self.index {
            match index.index_file(&path, package) {
                Ok(count) => {
                    debug!("Indexed {} symbols from {:?}", count, path);
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to index {:?}: {}", path, e);
                    Err(e)
                }
            }
        } else {
            // Try to recreate index if it failed before
            match SymbolIndex::open(&self.index_path) {
                Ok(mut idx) => {
                    let count = idx.index_file(&path, package)?;
                    debug!("Indexed {} symbols from {:?}", count, path);
                    self.index = Some(idx);
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to recreate user index: {}", e);
                    Err(e)
                }
            }
        }
    }

    /// Get index statistics
    pub fn stats(&self) -> Option<(usize, usize)> {
        self.index.as_ref().and_then(|idx| {
            idx.stats()
                .ok()
                .map(|s| (s.file_count, s.symbol_count))
        })
    }

    /// Look up a symbol in the user index
    pub fn lookup(&self, package: &str, symbol: &str) -> Result<Option<(PathBuf, u32, u32)>> {
        if let Some(ref index) = self.index {
            index.lookup(package, symbol)
        } else {
            Ok(None)
        }
    }

    /// Extract package from file content
    pub fn extract_package(text: &str) -> String {
        use zed_cl_indexer::SymbolExtractor;

        match SymbolExtractor::new() {
            Ok(mut extractor) => extractor
                .extract_package(text)
                .unwrap_or_else(|| "COMMON-LISP-USER".to_string()),
            Err(_) => "COMMON-LISP-USER".to_string(),
        }
    }
}
