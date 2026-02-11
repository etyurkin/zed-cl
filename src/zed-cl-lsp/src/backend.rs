/// LSP backend implementation

use common_rust::{MasterReplClient, ReplRequest};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use tracing::{debug, error, info};
use zeromq::{Socket, SocketSend};

use crate::document::DocumentTracker;
use crate::symbol_extractor::TreeSitterExtractor;
use crate::symbol_index::{SharedSymbolIndex, SymbolIndex};
use crate::user_index::UserIndexManager;

pub struct LispLspBackend {
    client: Client,
    master_repl: Arc<RwLock<MasterReplClient>>,
    documents: Arc<RwLock<DocumentTracker>>,
    symbol_index: SharedSymbolIndex,
    user_index: Arc<RwLock<UserIndexManager>>,
    workspace_roots: Arc<RwLock<Vec<Url>>>,
    shutdown_tx: Option<mpsc::UnboundedSender<()>>,
}

impl LispLspBackend {
    pub fn new(
        client: Client,
        endpoint: String,
    ) -> Self {
        // Use ~/.zed-cl/ for databases
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let db_dir = std::path::PathBuf::from(home).join(".zed-cl");

        // Get system index name from profile config
        let profile = common_rust::Config::get();
        let system_index_name = &profile.system_index;

        let index_paths = vec![db_dir.join(system_index_name)];
        let user_index_path = db_dir.join("user-index.db");

        info!("Using system index: {}", system_index_name);

        // Load symbol indexes (will be empty if db doesn't exist)
        let symbol_index = match SymbolIndex::new(index_paths) {
            Ok(idx) => {
                if !idx.is_empty() {
                    info!("System index loaded: {}", system_index_name);
                } else {
                    info!("System index not found: {} (goto-definition will work for workspace code only)", system_index_name);
                }
                Arc::new(RwLock::new(idx))
            }
            Err(e) => {
                error!("Failed to load system index: {}", e);
                Arc::new(RwLock::new(SymbolIndex::new(vec![]).unwrap()))
            }
        };

        // Create user index manager
        let user_index = Arc::new(RwLock::new(UserIndexManager::new(user_index_path)));

        // Create shutdown broadcast channel and spawn background thread with ZMQ PUB socket
        // The socket must be bound BEFORE kernels start, so they can connect to it
        let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel::<()>();

        tokio::spawn(async move {
            let mut socket = zeromq::PubSocket::new();
            match socket.bind("tcp://127.0.0.1:5557").await {
                Ok(_) => {
                    info!("Shutdown broadcast socket bound to tcp://127.0.0.1:5557");

                    // Wait for shutdown signal
                    shutdown_rx.recv().await;

                    info!("Received shutdown signal, broadcasting to kernels...");
                    let msg = zeromq::ZmqMessage::from("SHUTDOWN".as_bytes().to_vec());
                    if let Err(e) = socket.send(msg).await {
                        error!("Failed to send shutdown broadcast: {}", e);
                    } else {
                        info!("Shutdown broadcast sent");
                        // Give kernels time to receive
                        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                    }
                }
                Err(e) => {
                    error!("Failed to bind shutdown broadcast socket: {}", e);
                }
            }
        });

        Self {
            client,
            master_repl: Arc::new(RwLock::new(MasterReplClient::new(endpoint))),
            documents: Arc::new(RwLock::new(DocumentTracker::new())),
            symbol_index,
            user_index,
            workspace_roots: Arc::new(RwLock::new(Vec::new())),
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Check if a file URI is within any workspace root
    /// Format documentation string for better readability
    fn format_documentation(doc: &str) -> String {
        // Replace escaped tildes (~~) with single tildes (~)
        let mut text = doc.replace("~~", "~");

        // Apply text transformations BEFORE splitting into lines
        // This is important because ECL documentation often comes as one long line
        text = Self::format_inline_syntax(&text);

        // Now split into lines for processing
        let lines: Vec<&str> = text.split('\n').collect();
        let mut result = String::new();

        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            // Detect section headers: "Valid Options:", "Syntax:", etc.
            // If the header has content after it, split and format separately
            let header_keywords = ["Valid Options:", "Options:", "Syntax:", "Examples:", "Description:"];
            let mut found_header = false;

            for keyword in &header_keywords {
                if let Some(pos) = line.find(keyword) {
                    let before = &line[..pos];
                    let header = keyword;
                    let after = &line[pos + keyword.len()..].trim();

                    // Add any text before the header
                    if !before.is_empty() {
                        result.push_str(&Self::format_inline_syntax(before));
                        result.push(' ');
                    }

                    // Bold the header
                    result.push_str("**");
                    result.push_str(header);
                    result.push_str("**");

                    // If there's content after the header, format it separately
                    if !after.is_empty() {
                        result.push(' ');
                        // Check if it's a Lisp form
                        if after.starts_with('(') {
                            result.push_str("`");
                            result.push_str(after);
                            result.push_str("`");
                        } else {
                            result.push_str(&Self::format_inline_syntax(after));
                        }
                    }
                    result.push_str("\n\n");

                    found_header = true;
                    break;
                }
            }

            if found_header {
                i += 1;
                continue;
            }

            // Skip empty lines
            if line.is_empty() {
                result.push('\n');
                i += 1;
                continue;
            }

            // If line starts with ~, it's a format directive - format as code
            if line.starts_with('~') && line.len() < 50 {
                result.push_str(&format!("- `{}`\n", line));
                i += 1;
                continue;
            }

            // If line starts with '(' and ends with ')' and looks like a Lisp form
            // Format as inline code
            if line.starts_with('(') && (line.ends_with(')') || line.ends_with(")*")) {
                result.push_str("- `");
                result.push_str(line);
                result.push_str("`\n");
                i += 1;
                continue;
            }

            // Regular text (already has inline formatting applied from earlier)
            result.push_str(line);
            result.push('\n');
            i += 1;
        }

        // Collapse multiple newlines
        while result.contains("\n\n\n") {
            result = result.replace("\n\n\n", "\n\n");
        }

        result.trim().to_string()
    }

    /// Format inline Lisp syntax patterns in documentation text
    /// Wraps patterns like {var}, [&optional ...], {decl | doc}* in backticks
    fn format_inline_syntax(text: &str) -> String {
        use regex::Regex;

        let mut result = text.to_string();

        // Detect "The complete syntax of a lambda-list is:" and extract everything until next sentence
        // Lambda-lists end with *]) before "The doc-string" (may span multiple lines)
        if result.contains("The complete syntax of a lambda-list is:") {
            // Use (?s) for single-line mode (. matches newlines) and match across lines
            if let Ok(re) = Regex::new(r"(?s)The complete syntax of a lambda-list is:\s*(.+?\*\]\))\s+The\s+") {
                result = re.replace(&result, "The complete syntax of a lambda-list is:\n\n```lisp\n$1\n```\n\nThe ").to_string();
            }
        }

        // Add paragraph breaks before sentences that start with "The" after *])
        // May be on different lines
        if let Ok(re) = Regex::new(r"(?s)(\*\]\))\s+(The )") {
            result = re.replace_all(&result, "$1\n\n$2").to_string();
        }

        // Pattern to match:
        // - {var} or {decl | doc}* or {anything}*
        // - [&optional ...] or [init] or [svar]
        // - &rest, &optional, &key (lambda list keywords)
        let pattern = r"(\{[^}]+\}\*?|\[[^\]]+\]|&[a-z]+)";

        match Regex::new(pattern) {
            Ok(re) => {
                let result = re.replace_all(&result, "`$1`");
                result.to_string()
            }
            Err(_) => result
        }
    }

    async fn is_workspace_file(&self, uri: &Url) -> bool {
        let workspace_roots = self.workspace_roots.read().await;

        // If no workspace roots configured, accept all files (fallback)
        if workspace_roots.is_empty() {
            info!("No workspace roots configured, accepting all files (file: {})", uri);
            return true;
        }

        let uri_str = uri.as_str();

        // Check if URI starts with any workspace root
        for root in workspace_roots.iter() {
            if uri_str.starts_with(root.as_str()) {
                return true;
            }
        }

        info!("File {} is outside workspace roots: {:?}", uri, *workspace_roots);
        false
    }

    /// Parse a possibly package-qualified prefix.
    /// Returns (package_name, symbol_prefix, external_only)
    /// Example: "MY-APP:GET-" => (Some("MY-APP"), Some("GET-"), true)
    ///          "UTILS::FOO" => (Some("UTILS"), Some("FOO"), false)
    ///          "BAR" => (None, None, false)
    fn parse_fqn_prefix(prefix: &str) -> (Option<String>, Option<String>, bool) {
        if let Some(double_colon_pos) = prefix.find("::") {
            // Package::symbol (internal symbols)
            (
                Some(prefix[..double_colon_pos].to_string()),
                Some(prefix[double_colon_pos + 2..].to_string()),
                false,
            )
        } else if let Some(single_colon_pos) = prefix.find(':') {
            // Package:symbol (external symbols only)
            (
                Some(prefix[..single_colon_pos].to_string()),
                Some(prefix[single_colon_pos + 1..].to_string()),
                true,
            )
        } else {
            // No package qualifier
            (None, None, false)
        }
    }

    /// Create a snippet for a function with parameter placeholders
    /// If inside_paren is true, adds closing paren
    /// param_types: Optional list of (param_name, type_name) for type-based placeholder formatting
    fn create_function_snippet(&self, symbol: &str, source: &str, inside_paren: bool, param_types: Option<&Vec<(String, Option<String>)>>) -> Option<String> {
        // Parse parameter list from source like "(defun foo (x y z)...)"
        // Extract the parameter list
        let source = source.trim();

        // Find the parameter list - it's in parens after the function name
        let start = source.find('(')?;
        let after_defun = &source[start + 1..];

        // Skip the defun/defmacro keyword and function name
        let parts: Vec<&str> = after_defun.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }

        // Find the opening paren of the parameter list
        let rest = after_defun.find('(')? ;
        let param_start = &after_defun[rest + 1..];

        // Find the closing paren
        let param_end = param_start.find(')')?;
        let params_str = &param_start[..param_end];

        if params_str.trim().is_empty() || params_str.trim().to_uppercase() == "NIL" {
            // No parameters - just add closing paren if needed
            if inside_paren {
                return Some(format!("{}$0)", symbol.to_lowercase()));
            } else {
                return Some(symbol.to_lowercase());
            }
        }

        // Parse parameters and track keyword parameters
        let mut params: Vec<(&str, bool)> = Vec::new(); // (param_name, is_keyword)
        let mut is_keyword = false;

        for token in params_str.split_whitespace() {
            if token.starts_with('&') {
                // Track if we've entered keyword parameter territory
                if token.eq_ignore_ascii_case("&key") {
                    is_keyword = true;
                }
                // Skip lambda list keywords themselves
                continue;
            }
            params.push((token, is_keyword));
        }

        if params.is_empty() {
            if inside_paren {
                return Some(format!("{}$0)", symbol.to_lowercase()));
            } else {
                return Some(symbol.to_lowercase());
            }
        }

        // Build snippet with placeholders
        let mut snippet = symbol.to_lowercase();
        for (i, (param, is_key)) in params.iter().enumerate() {
            let param_lower = param.to_lowercase();

            // Only use quotes if we have explicit type information indicating string
            let needs_quotes = if let Some(types) = param_types {
                types.iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(param))
                    .and_then(|(_, type_name)| type_name.as_ref())
                    .map(|t| {
                        let t_upper = t.to_uppercase();
                        t_upper.contains("STRING") || t_upper.contains("SIMPLE-STRING")
                    })
                    .unwrap_or(false)
            } else {
                false
            };

            // Keyword parameters need the : prefix
            if *is_key {
                if needs_quotes {
                    snippet.push_str(&format!(" :{} \"${{{}:{}}}\"", param_lower, i + 1, param_lower));
                } else {
                    snippet.push_str(&format!(" :{} ${{{}:{}}}", param_lower, i + 1, param_lower));
                }
            } else {
                if needs_quotes {
                    snippet.push_str(&format!(" \"${{{}:{}}}\"", i + 1, param_lower));
                } else {
                    snippet.push_str(&format!(" ${{{}:{}}}", i + 1, param_lower));
                }
            }
        }

        if inside_paren {
            snippet.push_str("$0)");
        } else {
            snippet.push_str("$0");
        }

        Some(snippet)
    }
}

/// Convert byte offset to LSP Position (line and character)
/// SBCL provides character offset, we need to convert to line/character
fn offset_to_position(source: &str, offset: usize) -> Position {
    let mut line = 0;
    let mut character = 0;

    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }

    Position {
        line: line as u32,
        character: character as u32,
    }
}

impl LispLspBackend {
    /// Create Location from SBCL source file information
    fn create_external_location(
        &self,
        source_file: &str,
        source_character: Option<u32>,
        symbol_name: &str,
        symbol_kind: Option<&str>,
    ) -> Option<Location> {
        // Convert file path to URI
        let path = std::path::Path::new(source_file);
        let uri = Url::from_file_path(path).ok()?;

        // Read file content
        let content = std::fs::read_to_string(path).ok()?;

        // If we have a character offset, search for the symbol definition in nearby lines
        if let Some(char_offset) = source_character {
            let base_position = offset_to_position(&content, char_offset as usize);

            // Search for symbol name within +/- 20 lines of the offset
            let lines: Vec<&str> = content.lines().collect();
            let start_line = base_position.line.saturating_sub(20) as usize;
            let end_line = ((base_position.line + 20) as usize).min(lines.len());

            let symbol_lower = symbol_name.to_lowercase();

            // Build search patterns based on symbol kind
            let search_patterns: Vec<String> = if let Some(kind) = symbol_kind {
                match kind {
                    "macro" => vec![
                        format!("defmacro {}", symbol_lower),
                        format!("sb-xc:defmacro {}", symbol_lower),
                        format!("define-macro {}", symbol_lower),
                    ],
                    "function" => vec![
                        format!("defun {}", symbol_lower),
                        format!("sb-xc:defun {}", symbol_lower),
                        format!("define-list-map {}", symbol_lower),
                    ],
                    "special-operator" => vec![
                        format!("def-special-operator {}", symbol_lower),
                        format!("defspecial {}", symbol_lower),
                    ],
                    "variable" | "constant" => vec![
                        format!("defvar {}", symbol_lower),
                        format!("defparameter {}", symbol_lower),
                        format!("defconstant {}", symbol_lower),
                    ],
                    _ => vec![symbol_lower.clone()],
                }
            } else {
                vec![symbol_lower.clone()]
            };

            // Search for definition patterns first
            for (line_idx, line) in lines[start_line..end_line].iter().enumerate() {
                let actual_line = start_line + line_idx;
                let line_lower = line.to_lowercase();

                // Try to find definition patterns
                for pattern in &search_patterns {
                    if let Some(col) = line_lower.find(pattern) {
                        // Find where the symbol name actually starts in the pattern
                        let symbol_col = col + pattern.len() - symbol_lower.len();
                        return Some(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position {
                                    line: actual_line as u32,
                                    character: symbol_col as u32
                                },
                                end: Position {
                                    line: actual_line as u32,
                                    character: symbol_col as u32
                                },
                            },
                        });
                    }
                }
            }

            // If definition patterns not found nearby, search the entire file
            // This helps when SBCL reports the wrong file or wrong offset
            if !search_patterns.is_empty() {
                for (line_idx, line) in lines.iter().enumerate() {
                    let line_lower = line.to_lowercase();

                    // Try to find definition patterns in the entire file
                    for pattern in &search_patterns {
                        if let Some(col) = line_lower.find(pattern) {
                            // Find where the symbol name actually starts in the pattern
                            let symbol_col = col + pattern.len() - symbol_lower.len();
                            return Some(Location {
                                uri: uri.clone(),
                                range: Range {
                                    start: Position {
                                        line: line_idx as u32,
                                        character: symbol_col as u32
                                    },
                                    end: Position {
                                        line: line_idx as u32,
                                        character: symbol_col as u32
                                    },
                                },
                            });
                        }
                    }
                }

                // If we were looking for a specific definition pattern (like "defmacro cond")
                // and didn't find it anywhere in the file, the source location is unreliable.
                // Return None instead of a wrong location.
                return None;
            }

            // Fallback: search for the symbol name as a complete word
            for (line_idx, line) in lines[start_line..end_line].iter().enumerate() {
                let actual_line = start_line + line_idx;
                let line_lower = line.to_lowercase();

                // Look for the symbol name as a complete word
                if let Some(col) = line_lower.find(&symbol_lower) {
                    // Check if it's a word boundary (not part of a larger word)
                    let before_ok = col == 0 || !line.chars().nth(col.saturating_sub(1))
                        .map(|c| c.is_alphanumeric() || c == '-' || c == '*' || c == '+')
                        .unwrap_or(false);

                    let after_idx = col + symbol_lower.len();
                    let after_ok = after_idx >= line.len() || !line.chars().nth(after_idx)
                        .map(|c| c.is_alphanumeric() || c == '-' || c == '*' || c == '+')
                        .unwrap_or(false);

                    if before_ok && after_ok {
                        return Some(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position {
                                    line: actual_line as u32,
                                    character: col as u32
                                },
                                end: Position {
                                    line: actual_line as u32,
                                    character: col as u32
                                },
                            },
                        });
                    }
                }
            }

            // If symbol not found nearby, use the offset position as-is
            return Some(Location {
                uri,
                range: Range {
                    start: base_position,
                    end: base_position,
                },
            });
        }

        // Fallback to TreeSitter search if no character offset available
        let mut extractor = TreeSitterExtractor::new().ok()?;
        let definitions = extractor.find_definitions(&content);

        // Look for matching symbol definition
        if let Some((_, pos)) = definitions.into_iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(symbol_name)) {
            return Some(Location {
                uri,
                range: Range {
                    start: pos,
                    end: pos,
                },
            });
        }

        // Last resort: start of file
        Some(Location {
            uri,
            range: Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: 0, character: 0 },
            },
        })
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for LispLspBackend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        info!("Initializing Rust LSP server for Common Lisp");

        // Store workspace roots for filtering user files
        if let Some(roots) = params.workspace_folders {
            let mut workspace_roots = self.workspace_roots.write().await;
            *workspace_roots = roots.into_iter().map(|folder| folder.uri).collect();
            info!("Workspace roots: {:?}", *workspace_roots);
        } else if let Some(root_uri) = params.root_uri {
            let mut workspace_roots = self.workspace_roots.write().await;
            workspace_roots.push(root_uri.clone());
            info!("Workspace root: {:?}", root_uri);
        }

        // Start master REPL if needed (async, non-blocking)
        let repl = self.master_repl.read().await;

        // Quick ping with 100ms timeout
        info!("Quick ping to check if master REPL is running...");
        if !repl.is_connected() {
            info!("Master REPL not responding, starting in background...");
            // Spawn REPL asynchronously without waiting
            repl.try_start_master_repl();
            info!("Master REPL spawn initiated (will be ready soon)");
        } else {
            info!("Master REPL is already running");
        }
        drop(repl);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "(".to_string(),
                        ":".to_string(),
                        "-".to_string(),
                        "*".to_string(),
                        "+".to_string(),
                    ]),
                    ..Default::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "cl-zed-lsp (Rust)".to_string(),
                version: Some("0.1.0".to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        info!("LSP server initialized");
        self.client
            .log_message(MessageType::INFO, "Common Lisp LSP server ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down LSP server");

        // Signal the background thread to broadcast shutdown
        if let Some(ref tx) = self.shutdown_tx {
            if let Err(e) = tx.send(()) {
                error!("Failed to send shutdown signal to broadcast thread: {}", e);
            } else {
                info!("Shutdown signal sent to broadcast thread");
                // Give time for broadcast to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            }
        }

        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();
        debug!("Document opened: {}", uri);

        self.documents
            .write()
            .await
            .open(params.text_document.uri, text.clone());

        // Only index files within workspace (skip external files like SBCL sources)
        if !self.is_workspace_file(&uri).await {
            info!("Skipping index for file outside workspace: {}", uri);
            return;
        }

        // Index user file for goto-definition
        let package = UserIndexManager::extract_package(&text);
        let mut user_index = self.user_index.write().await;
        if let Err(e) = user_index.index_file(&uri, &package) {
            error!("Failed to index file {}: {}", uri, e);
        } else {
            debug!("Indexed file {} (package: {})", uri, package);
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        debug!("Document changed: {}", uri);
        self.documents
            .write()
            .await
            .change(params.text_document.uri, params.content_changes);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        debug!("Document saved: {}", uri);

        // Only index files within workspace (skip external files like SBCL sources)
        if !self.is_workspace_file(&uri).await {
            info!("Skipping index for file outside workspace: {}", uri);
            return;
        }

        // Re-index the file on save
        let documents = self.documents.read().await;
        if let Some(text) = documents.get(&uri) {
            let package = UserIndexManager::extract_package(text);
            let mut user_index = self.user_index.write().await;
            if let Err(e) = user_index.index_file(&uri, &package) {
                error!("Failed to re-index file {}: {}", uri, e);
            } else {
                debug!("Re-indexed file {} (package: {})", uri, package);

                // Log stats
                if let Some((file_count, symbol_count)) = user_index.stats() {
                    self.client
                        .log_message(
                            MessageType::INFO,
                            format!("📑 User index: {} files, {} symbols", file_count, symbol_count),
                        )
                        .await;
                }
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        debug!("Document closed: {}", params.text_document.uri);
        self.documents.write().await.close(&params.text_document.uri);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Log to Zed's UI so we can see it
        self.client
            .log_message(
                MessageType::INFO,
                format!("Hover at line {} char {}", position.line, position.character),
            )
            .await;

        debug!("Hover request at {}:{}:{}", uri, position.line, position.character);

        // Get document text
        let documents = self.documents.read().await;
        let text = match documents.get(&uri) {
            Some(text) => text,
            None => {
                self.client.log_message(MessageType::INFO, "Document not found").await;
                return Ok(None);
            }
        };

        // Extract symbol at position
        let mut extractor = match TreeSitterExtractor::new() {
            Ok(e) => e,
            Err(e) => {
                self.client.log_message(MessageType::ERROR, format!("Tree-sitter error: {}", e)).await;
                error!("Failed to create tree-sitter extractor: {}", e);
                return Ok(None);
            }
        };

        let extraction_result = extractor.symbol_with_package(text, position);

        // Log the extraction result with debug info from extractor
        self.client.log_message(
            MessageType::INFO,
            format!("Extraction result: {:?}", extraction_result)
        ).await;

        // Try to get the line for debugging
        let lines: Vec<&str> = text.lines().collect();
        if let Some(line) = lines.get(position.line as usize) {
            self.client.log_message(
                MessageType::INFO,
                format!("Line {}: {}", position.line, line)
            ).await;
        }

        let (symbol_name, package_name) = match extraction_result {
            Some((sym, pkg, parents)) => {
                self.client.log_message(
                    MessageType::INFO,
                    format!("Found symbol: {} package: {:?}, parents: {:?}", sym, pkg, parents)
                ).await;
                (sym, pkg)
            }
            None => {
                self.client.log_message(MessageType::INFO, "No symbol found at position").await;
                debug!("No symbol found at position");
                return Ok(None);
            }
        };

        debug!("Looking up symbol: {} in package: {:?}",
               symbol_name, package_name);

        // Query master REPL for symbol info
        let request = ReplRequest::SymbolInfo {
            id: String::new(),
            symbol: symbol_name.clone(),
            package: package_name,
        };

        let mut repl = self.master_repl.write().await;
        debug!("Sending request to master REPL");
        match repl.send_request(request) {
            Ok(response) => {
                debug!("Got response from master REPL: {:?}", response);

                // Convert response to Hover markdown
                use common_rust::ResponseData;
                match response.data {
                    ResponseData::SymbolInfo(info) => {
                        let mut markdown = String::new();

                        // Add symbol header with kind
                        markdown.push_str(&format!("**{}** _{}_\n\n", info.symbol, info.kind));

                        // Add package info
                        markdown.push_str(&format!("Package: `{}`\n\n", info.package));

                        // Add parameter types if available
                        if let Some(ref param_types) = info.param_types {
                            if !param_types.is_empty() {
                                markdown.push_str("**Parameters:**\n");
                                for (param_name, type_name) in param_types {
                                    if let Some(ref type_str) = type_name {
                                        markdown.push_str(&format!("- `{}`: `{}`\n", param_name, type_str));
                                    } else {
                                        markdown.push_str(&format!("- `{}`\n", param_name));
                                    }
                                }
                                markdown.push_str("\n");
                            }
                        }

                        // Add source signature if available
                        if let Some(ref source) = info.source {
                            markdown.push_str("```lisp\n");
                            markdown.push_str(source);
                            markdown.push_str("\n```\n\n");
                        }

                        // Add documentation if available
                        if let Some(ref doc) = info.doc {
                            // Format documentation for better readability
                            let formatted_doc = Self::format_documentation(doc);
                            markdown.push_str(&formatted_doc);
                        }

                        Ok(Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: markdown,
                            }),
                            range: None,
                        }))
                    }
                    ResponseData::Error { error } => {
                        debug!("Master REPL returned error: {}", error);
                        Ok(None)
                    }
                    _ => {
                        debug!("Unexpected response type");
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                error!("Failed to query master REPL: {}", e);
                Ok(None)
            }
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        debug!("Completion request at {}:{}:{}", uri, position.line, position.character);

        // Get document text
        let documents = self.documents.read().await;
        let text = match documents.get(&uri) {
            Some(text) => text,
            None => return Ok(None),
        };

        // Extract prefix being typed
        let mut extractor = match TreeSitterExtractor::new() {
            Ok(e) => e,
            Err(e) => {
                error!("Failed to create tree-sitter extractor: {}", e);
                return Ok(None);
            }
        };

        let prefix = match extractor.prefix_at_position(text, position) {
            Some(p) => p,
            None => return Ok(None),
        };

        debug!("Completion prefix: {}", prefix);

        // Check if we're inside a list (after an opening paren)
        let lines: Vec<&str> = text.lines().collect();
        let inside_paren = if (position.line as usize) < lines.len() {
            let line = lines[position.line as usize];
            let before_cursor = &line[..position.character.saturating_sub(prefix.len() as u32) as usize];
            // Check if there's an opening paren before the prefix
            before_cursor.trim_end().ends_with('(')
        } else {
            false
        };

        debug!("Inside paren: {}", inside_paren);

        // Parse prefix for package qualification: "pkg:sym" or "pkg::sym"
        let (package_name, symbol_prefix, _external_only) = Self::parse_fqn_prefix(&prefix);

        // Calculate the range of the prefix for text replacement
        // If we have a package qualifier, only replace the symbol part after ::
        // Special case: empty package name (":symbol") means keyword, replace entire prefix including :
        let prefix_range = if package_name.is_some() && package_name.as_ref().map(|s| !s.is_empty()).unwrap_or(false) {
            // Find the position after :: or :
            let qualifier_end = if let Some(pos) = prefix.rfind("::") {
                pos + 2
            } else if let Some(pos) = prefix.rfind(':') {
                pos + 1
            } else {
                0
            };

            let symbol_start = Position {
                line: position.line,
                character: position.character - (prefix.len() - qualifier_end) as u32,
            };
            tower_lsp::lsp_types::Range {
                start: symbol_start,
                end: position,
            }
        } else {
            // No package qualifier, replace entire prefix
            let prefix_start = Position {
                line: position.line,
                character: position.character - prefix.len() as u32,
            };
            tower_lsp::lsp_types::Range {
                start: prefix_start,
                end: position,
            }
        };

        debug!("Parsed FQN: package={:?}, symbol={:?}", package_name, symbol_prefix);

        // Query master REPL for matching symbols
        let request = ReplRequest::ListSymbols {
            id: String::new(),
            prefix: Some(symbol_prefix.unwrap_or(prefix.clone())),
        };

        let mut repl = self.master_repl.write().await;
        match repl.send_request(request) {
            Ok(response) => {
                // Convert response to CompletionItems
                use common_rust::ResponseData;
                match response.data {
                    ResponseData::SymbolList { symbols } => {
                        let mut items: Vec<CompletionItem> = Vec::new();

                        // We don't need to guess - we have package info for each symbol!

                        if package_name.is_none() {
                            // Collect unique package names from symbols
                            let mut seen_packages = std::collections::HashSet::new();
                            for sym_info in &symbols {
                                if !seen_packages.contains(&sym_info.package) {
                                    // Check if package name starts with prefix
                                    if sym_info.package.to_uppercase().starts_with(&prefix.to_uppercase()) {
                                        seen_packages.insert(sym_info.package.clone());

                                        // Add package completion item (single : so user can type second : for completions)
                                        let pkg_lower = sym_info.package.to_lowercase();
                                        items.push(CompletionItem {
                                            label: format!("[{}]", pkg_lower),
                                            kind: Some(CompletionItemKind::MODULE),
                                            detail: Some("package".to_string()),
                                            filter_text: Some(pkg_lower.clone()),
                                            text_edit: Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(
                                                tower_lsp::lsp_types::TextEdit {
                                                    range: prefix_range,
                                                    new_text: format!("{}:", pkg_lower),
                                                }
                                            )),
                                            sort_text: Some(format!("1{}", pkg_lower)), // Packages after user symbols
                                            ..Default::default()
                                        });
                                    }
                                }
                            }
                        }

                        // Filter symbols by package if specified, OR by prefix matching
                        let filtered_symbols: Vec<_> = if let Some(ref pkg) = package_name {
                            // Empty package ("") means keywords - show symbols from KEYWORD package
                            if pkg.is_empty() {
                                symbols.into_iter()
                                    .filter(|info| info.package.eq_ignore_ascii_case("KEYWORD"))
                                    .collect()
                            } else {
                                // User typed "pkg::" - show only symbols from that package
                                symbols.into_iter()
                                    .filter(|info| info.package.eq_ignore_ascii_case(pkg))
                                    .collect()
                            }
                        } else {
                            // No package qualifier - include:
                            // 1. Symbols whose name starts with prefix
                            // 2. ALL symbols from packages whose name starts with prefix
                            let prefix_upper = prefix.to_uppercase();
                            let mut matching_packages = std::collections::HashSet::new();

                            // Find packages that match the prefix
                            for sym_info in &symbols {
                                if sym_info.package.to_uppercase().starts_with(&prefix_upper) {
                                    matching_packages.insert(sym_info.package.clone());
                                }
                            }

                            symbols.into_iter()
                                .filter(|info| {
                                    // Include if symbol name matches OR package matches
                                    info.symbol.to_uppercase().starts_with(&prefix_upper) ||
                                    matching_packages.contains(&info.package)
                                })
                                .collect()
                        };

                        // Debug: log first 5 symbols to see the order from REPL
                        for (i, sym) in filtered_symbols.iter().take(5).enumerate() {
                            debug!("Symbol {}: {} from package {}", i, sym.symbol, sym.package);
                        }

                        let symbol_items: Vec<CompletionItem> = filtered_symbols
                            .into_iter()
                            .enumerate()
                            .map(|(index, info)| {
                                let kind = match info.kind.as_str() {
                                    "function" | "macro" => CompletionItemKind::FUNCTION,
                                    "variable" | "constant" => CompletionItemKind::VARIABLE,
                                    "class" => CompletionItemKind::CLASS,
                                    "package" => CompletionItemKind::MODULE,
                                    _ => CompletionItemKind::TEXT,
                                };

                                // Sorting is handled on REPL side (KEYWORD, COMMON-LISP, others)
                                // Use padded index as sort_text to preserve exact order from REPL
                                // Format: "00000000", "00000001", "00000002", etc.
                                let sort_text = format!("{:08}{}", index, info.symbol.to_lowercase());

                                // Determine if we need to prepend package name
                                // Only add package prefix when inside a function call (after "(")
                                // AND when:
                                // 1. User hasn't already typed a package qualifier (package_name.is_none())
                                // 2. Symbol is NOT from COMMON-LISP or COMMON-LISP-USER (those don't need qualification)
                                let is_system_package = matches!(
                                    info.package.as_str(),
                                    "COMMON-LISP" | "COMMON-LISP-USER" | "CL" | "KEYWORD"
                                );
                                let needs_package_prefix = inside_paren && package_name.is_none() && !is_system_package;

                                // For functions, try to create a snippet with parameter placeholders
                                // Only create snippets when inside a function call (after "(")
                                let (mut insert_text, insert_text_format) = if inside_paren && (info.kind == "function" || info.kind == "macro") {
                                    if let Some(ref source) = info.source {
                                        if let Some(snippet) = self.create_function_snippet(
                                            &info.symbol,
                                            source,
                                            inside_paren,
                                            info.param_types.as_ref()
                                        ) {
                                            (snippet, Some(InsertTextFormat::SNIPPET))
                                        } else {
                                            (info.symbol.to_lowercase(), None)
                                        }
                                    } else {
                                        (info.symbol.to_lowercase(), None)
                                    }
                                } else {
                                    (info.symbol.to_lowercase(), None)
                                };

                                // Prepend package:: if needed
                                if needs_package_prefix {
                                    insert_text = format!("{}::{}", info.package.to_lowercase(), insert_text);
                                }

                                // Prepend : for keywords
                                if info.package.to_uppercase() == "KEYWORD" {
                                    insert_text = format!(":{}", insert_text);
                                }

                                // Extract parameter list from source for detail, with types if available
                                let detail = if let Some(ref source) = info.source {
                                    // Try to extract parameter list from source like "(defun foo (a b c) ...)"
                                    if let Some(params_start) = source.find('(').and_then(|p1| {
                                        source[p1+1..].find('(').map(|p2| p1 + 1 + p2)
                                    }) {
                                        if let Some(params_end) = source[params_start..].find(')') {
                                            let params = &source[params_start..params_start + params_end + 1];

                                            // If we have type information, format it nicely
                                            if let Some(ref param_types) = info.param_types {
                                                if !param_types.is_empty() {
                                                    // Build typed parameter list: (name:type ...)
                                                    let typed_params: Vec<String> = param_types.iter()
                                                        .map(|(name, type_opt)| {
                                                            if let Some(type_name) = type_opt {
                                                                format!("{}:{}", name.to_lowercase(), type_name.to_lowercase())
                                                            } else {
                                                                name.to_lowercase()
                                                            }
                                                        })
                                                        .collect();
                                                    format!("{} ({})", info.kind, typed_params.join(" "))
                                                } else {
                                                    format!("{} {}", info.kind, params)
                                                }
                                            } else {
                                                format!("{} {}", info.kind, params)
                                            }
                                        } else {
                                            info.kind.clone()
                                        }
                                    } else {
                                        info.kind.clone()
                                    }
                                } else {
                                    info.kind.clone()
                                };

                                // Don't set sort_text - rely on the order from REPL
                                // Symbols are already sorted by REPL: KEYWORD, COMMON-LISP, others

                                // Format label and filter text - keywords get : prefix
                                let (label, filter_text_value) = if info.package.to_uppercase() == "KEYWORD" {
                                    (
                                        format!("[{}] :{}", info.package.to_lowercase(), info.symbol.to_lowercase()),
                                        format!(":{}", info.symbol.to_lowercase())
                                    )
                                } else {
                                    (
                                        format!("[{}] {}", info.package.to_lowercase(), info.symbol.to_lowercase()),
                                        info.symbol.to_lowercase()
                                    )
                                };

                                debug!("Creating completion: label={}, filter_text={}, insert_text={}", label, filter_text_value, insert_text);

                                CompletionItem {
                                    label,
                                    kind: Some(kind),
                                    detail: Some(detail),
                                    documentation: info.doc.map(|doc| {
                                        Documentation::MarkupContent(MarkupContent {
                                            kind: MarkupKind::Markdown,
                                            value: doc,
                                        })
                                    }),
                                    // Set filter_text to symbol name for matching (with : for keywords)
                                    filter_text: Some(filter_text_value),
                                    text_edit: Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(
                                        tower_lsp::lsp_types::TextEdit {
                                            range: prefix_range,
                                            new_text: insert_text.clone(),
                                        }
                                    )),
                                    insert_text_format,
                                    // Use index as sort_text to preserve REPL order
                                    sort_text: Some(sort_text),
                                    ..Default::default()
                                }
                            })
                            .collect();

                        // Append symbol items to package items
                        items.extend(symbol_items);

                        debug!("Returning {} completion items", items.len());
                        Ok(Some(CompletionResponse::Array(items)))
                    }
                    _ => {
                        debug!("Unexpected response type for list-symbols");
                        Ok(Some(CompletionResponse::Array(vec![])))
                    }
                }
            }
            Err(e) => {
                error!("Failed to query master REPL: {}", e);
                Ok(None)
            }
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        self.client
            .log_message(
                MessageType::INFO,
                format!("Definition request at {}:{}", position.line, position.character),
            )
            .await;

        // Get document text
        let documents = self.documents.read().await;
        let text = match documents.get(&uri) {
            Some(text) => text,
            None => {
                self.client.log_message(MessageType::INFO, "Document not found for goto_definition").await;
                return Ok(None);
            }
        };
        self.client.log_message(MessageType::INFO, format!("Got document text, length: {}", text.len())).await;

        // Extract symbol at cursor position
        let mut extractor = match TreeSitterExtractor::new() {
            Ok(e) => e,
            Err(e) => {
                self.client.log_message(MessageType::ERROR, format!("Failed to create extractor: {}", e)).await;
                return Ok(None);
            }
        };

        self.client.log_message(MessageType::INFO, "Created TreeSitter extractor").await;

        let (symbol_name, package_name) = match extractor.symbol_with_package(text, position) {
            Some((sym, pkg, _)) => (sym, pkg),
            None => {
                self.client.log_message(MessageType::INFO, "No symbol found at position").await;
                return Ok(None);
            }
        };

        self.client.log_message(MessageType::INFO, format!("Looking for definition of: {} in package: {:?}", symbol_name, package_name)).await;

        // TIER 0a: Try user index first (smaller, user's code)
        if let Some(ref pkg) = package_name {
            self.client.log_message(MessageType::INFO, format!("Searching user index for {}::{}...", pkg, symbol_name)).await;

            let user_index = self.user_index.read().await;
            match user_index.lookup(pkg, &symbol_name) {
                Ok(Some((path, line, character))) => {
                    if let Ok(uri) = Url::from_file_path(&path) {
                        self.client.log_message(MessageType::INFO, format!("Found in user index: {:?}", path)).await;
                        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                            uri,
                            range: Range {
                                start: Position { line, character },
                                end: Position { line, character },
                            },
                        })));
                    }
                }
                Ok(None) => {
                    self.client.log_message(MessageType::INFO, "Not found in user index").await;
                }
                Err(e) => {
                    self.client.log_message(MessageType::ERROR, format!("User index lookup error: {}", e)).await;
                }
            }
        }

        // TIER 0b: Try SBCL/library index (pre-indexed symbols)
        // For SBCL symbols, try both the specified package and SB-IMPL (where many CL symbols are actually defined)
        let packages_to_try: Vec<&str> = if let Some(ref pkg) = package_name {
            vec![pkg.as_str()]
        } else {
            // No package specified - try COMMON-LISP first, then SB-IMPL (SBCL internal)
            vec!["COMMON-LISP", "SB-IMPL"]
        };

        for pkg in packages_to_try {
            self.client.log_message(MessageType::INFO, format!("Searching SBCL index for {}::{}...", pkg, symbol_name)).await;

            let index = self.symbol_index.read().await;
            match index.lookup(pkg, &symbol_name) {
                Ok(Some(location)) => {
                    self.client.log_message(MessageType::INFO, format!("Found in SBCL index: {:?}", location)).await;
                    return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                }
                Ok(None) => {
                    self.client.log_message(MessageType::INFO, format!("Not found in SBCL index for package {}", pkg)).await;
                }
                Err(e) => {
                    self.client.log_message(MessageType::ERROR, format!("SBCL index lookup error: {}", e)).await;
                }
            }
        }

        // TIER 1: Try local definition (fast, no REPL query needed)
        self.client.log_message(MessageType::INFO, "Searching local definitions...").await;

        // Search manually here so we can log
        let mut extractor = match TreeSitterExtractor::new() {
            Ok(e) => e,
            Err(e) => {
                self.client.log_message(MessageType::ERROR, format!("Failed to create extractor: {}", e)).await;
                self.client.log_message(MessageType::INFO, "Not found locally, querying REPL...").await;
                // Skip to REPL query
                let request = ReplRequest::SymbolInfo {
                    id: String::new(),
                    symbol: symbol_name.clone(),
                    package: package_name,
                };

                self.client.log_message(MessageType::INFO, format!("Sending REPL request for symbol: {}", symbol_name)).await;

                let mut repl = self.master_repl.write().await;
                match repl.send_request(request) {
                    Ok(response) => {
                        self.client.log_message(MessageType::INFO, "Got REPL response").await;
                        use common_rust::ResponseData;
                        match response.data {
                            ResponseData::SymbolInfo(info) => {
                                self.client.log_message(MessageType::INFO, format!("SymbolInfo: source_file={:?}, source_char={:?}", info.source_file, info.source_character)).await;
                                if let Some(ref source_file) = info.source_file {
                                    // Check for non-empty source file
                                    if !source_file.is_empty() {
                                        self.client.log_message(MessageType::INFO, format!("Found external definition in {}", source_file)).await;
                                        if let Some(location) =
                                            self.create_external_location(source_file, info.source_character, &symbol_name, Some(&info.kind))
                                        {
                                            self.client.log_message(MessageType::INFO, format!("Returning location: {:?}", location)).await;
                                            return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                                        }
                                        self.client.log_message(MessageType::INFO, "create_external_location returned None").await;
                                    } else {
                                        self.client.log_message(MessageType::INFO, "Source file is empty - symbol was evaluated without file context").await;
                                    }
                                }
                                self.client.log_message(MessageType::INFO, "Symbol info found but no source location").await;
                                return Ok(None);
                            }
                            ResponseData::Error { error } => {
                                self.client.log_message(MessageType::ERROR, format!("Master REPL returned error: {}", error)).await;
                                return Ok(None);
                            }
                            _ => {
                                self.client.log_message(MessageType::INFO, "Unexpected response type").await;
                                return Ok(None);
                            }
                        }
                    }
                    Err(e) => {
                        self.client.log_message(MessageType::ERROR, format!("Failed to query master REPL: {}", e)).await;
                        return Ok(None);
                    }
                }
            }
        };

        self.client.log_message(MessageType::INFO, "TreeSitter extractor created successfully").await;

        // Try to parse the source
        let tree = extractor.parse(text);
        if tree.is_none() {
            self.client.log_message(MessageType::ERROR, "TreeSitter parse returned None!").await;
        } else {
            self.client.log_message(MessageType::INFO, "TreeSitter parse successful").await;
        }

        let definitions = extractor.find_definitions(text);
        self.client.log_message(MessageType::INFO, format!("Found {} definitions in file", definitions.len())).await;

        let def_names: Vec<String> = definitions.iter().map(|(n, _)| n.clone()).collect();
        self.client.log_message(MessageType::INFO, format!("Definition names: {:?}", def_names)).await;
        self.client.log_message(MessageType::INFO, format!("Looking for: {}", symbol_name)).await;

        let found_def = definitions.into_iter().find(|(name, _)| {
            name.eq_ignore_ascii_case(&symbol_name)
        });

        if let Some((found_name, local_pos)) = found_def {
            self.client.log_message(MessageType::INFO, format!("Matched '{}' -> position {:?}", found_name, local_pos)).await;
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: Range {
                    start: local_pos,
                    end: local_pos,
                },
            })));
        }

        self.client.log_message(MessageType::INFO, "Not found locally, querying REPL...").await;

        // TIER 2: Not found locally - query master REPL for external definition
        let request = ReplRequest::SymbolInfo {
            id: String::new(),
            symbol: symbol_name.clone(),
            package: package_name,
        };

        self.client.log_message(MessageType::INFO, format!("Sending REPL request for symbol: {}", symbol_name)).await;

        let mut repl = self.master_repl.write().await;
        match repl.send_request(request) {
            Ok(response) => {
                self.client.log_message(MessageType::INFO, "Got REPL response").await;
                use common_rust::ResponseData;
                match response.data {
                    ResponseData::SymbolInfo(info) => {
                        self.client.log_message(MessageType::INFO, format!("SymbolInfo: source_file={:?}, source_char={:?}", info.source_file, info.source_character)).await;
                        if let Some(ref source_file) = info.source_file {
                            // Check for non-empty source file
                            if !source_file.is_empty() {
                                self.client.log_message(MessageType::INFO, format!("Found external definition in {}", source_file)).await;
                                if let Some(location) =
                                    self.create_external_location(source_file, info.source_character, &symbol_name, Some(&info.kind))
                                {
                                    self.client.log_message(MessageType::INFO, format!("Returning location: {:?}", location)).await;
                                    return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                                }
                                self.client.log_message(MessageType::INFO, "create_external_location returned None").await;
                            } else {
                                self.client.log_message(MessageType::INFO, "Source file is empty - symbol was evaluated without file context").await;
                            }
                        }
                        self.client.log_message(MessageType::INFO, "Symbol info found but no source location").await;
                        Ok(None)
                    }
                    ResponseData::Error { error } => {
                        self.client.log_message(MessageType::ERROR, format!("Master REPL returned error: {}", error)).await;
                        Ok(None)
                    }
                    _ => {
                        self.client.log_message(MessageType::INFO, "Unexpected response type").await;
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                self.client.log_message(MessageType::ERROR, format!("Failed to query master REPL: {}", e)).await;
                Ok(None)
            }
        }
    }
}
