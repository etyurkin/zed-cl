/// Protocol definitions for communicating with the master REPL
///
/// The master REPL accepts s-expression based requests over a Unix socket
/// and returns s-expression responses.

use serde::{Deserialize, Serialize};

/// Request types sent to master REPL
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ReplRequest {
    /// Get information about a symbol
    SymbolInfo {
        id: String,
        symbol: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<String>,
    },

    /// List all symbols (optionally filtered by prefix)
    ListSymbols {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
    },

    /// Evaluate Lisp code
    Eval {
        id: String,
        code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_line: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_character: Option<u32>,
    },

    /// Load a file
    LoadFile {
        id: String,
        path: String,
    },

    /// Set current file being edited (for source tracking)
    SetCurrentFile {
        id: String,
        path: String,
    },
}

/// Response from master REPL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplResponse {
    pub id: String,

    #[serde(flatten)]
    pub data: ResponseData,
}

/// Response data variants
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseData {
    SymbolInfo(SymbolInfo),
    SymbolList { symbols: Vec<SymbolInfo> },
    EvalResult {
        output: String,
        values: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        traceback: Option<String>,
        /// Rich display data (MIME-typed outputs for Jupyter)
        #[serde(skip_serializing_if = "Option::is_none")]
        displays: Option<Vec<DisplayData>>,
    },
    LoadResult {
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Error { error: String },
}

/// Rich display data for Jupyter (images, HTML, tables, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayData {
    /// MIME type -> data mapping
    /// Examples: "text/html", "image/png", "application/json", "text/markdown"
    pub data: std::collections::HashMap<String, String>,

    /// Optional metadata for the display
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Information about a Lisp symbol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub symbol: String,
    pub package: String,
    pub kind: String, // "function", "variable", "class", etc.

    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", rename = "param-types")]
    pub param_types: Option<Vec<(String, Option<String>)>>,

    #[serde(skip_serializing_if = "Option::is_none", rename = "source-file")]
    pub source_file: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", rename = "source-line")]
    pub source_line: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none", rename = "source-character")]
    pub source_character: Option<u32>,
}

impl ReplRequest {
    /// Convert request to s-expression string for sending to REPL
    pub fn to_sexp(&self) -> String {
        match self {
            ReplRequest::SymbolInfo { id, symbol, package } => {
                if let Some(pkg) = package {
                    format!("(:type \"symbol-info\" :id \"{}\" :symbol \"{}\" :package \"{}\")",
                           id, symbol, pkg)
                } else {
                    format!("(:type \"symbol-info\" :id \"{}\" :symbol \"{}\")", id, symbol)
                }
            }
            ReplRequest::ListSymbols { id, prefix } => {
                if let Some(pfx) = prefix {
                    format!("(:type \"list-symbols\" :id \"{}\" :prefix \"{}\")", id, pfx)
                } else {
                    format!("(:type \"list-symbols\" :id \"{}\")", id)
                }
            }
            ReplRequest::Eval { id, code, package, file_path, file_line, file_character } => {
                let escaped_code = code.replace("\\", "\\\\").replace("\"", "\\\"");
                let mut parts = vec![
                    format!(":type \"eval\""),
                    format!(":id \"{}\"", id),
                    format!(":code \"{}\"", escaped_code),
                ];
                if let Some(pkg) = package {
                    parts.push(format!(":package \"{}\"", pkg));
                }
                if let Some(path) = file_path {
                    let escaped_path = path.replace("\\", "\\\\").replace("\"", "\\\"");
                    parts.push(format!(":file-path \"{}\"", escaped_path));
                }
                if let Some(line) = file_line {
                    parts.push(format!(":file-line {}", line));
                }
                if let Some(character) = file_character {
                    parts.push(format!(":file-character {}", character));
                }
                format!("({})", parts.join(" "))
            }
            ReplRequest::LoadFile { id, path } => {
                format!("(:type \"load-file\" :id \"{}\" :path \"{}\")", id, path)
            }
            ReplRequest::SetCurrentFile { id, path } => {
                let escaped_path = path.replace("\\", "\\\\").replace("\"", "\\\"");
                format!("(:type \"set-current-file\" :id \"{}\" :path \"{}\")", id, escaped_path)
            }
        }
    }
}
