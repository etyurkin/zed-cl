/// LSP backend implementation

use common_rust::{MasterReplClient, ReplRequest};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use tracing::{debug, error, info};

use crate::document::DocumentTracker;
use crate::symbol_extractor::TreeSitterExtractor;
use crate::symbol_index::{SharedSymbolIndex, SymbolIndex};
use crate::user_index::UserIndexManager;

pub struct LispLspBackend {
    client: Client,
    master_repl: Arc<RwLock<MasterReplClient>>,
    documents: Arc<RwLock<DocumentTracker>>,
    extractor: Mutex<TreeSitterExtractor>,
    symbol_index: SharedSymbolIndex,
    user_index: Arc<RwLock<UserIndexManager>>,
    workspace_roots: Arc<RwLock<Vec<Url>>>,
}

impl LispLspBackend {
    pub fn new(client: Client) -> Self {
        let db_dir = common_rust::data_dir();

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

        Self {
            client,
            master_repl: Arc::new(RwLock::new(MasterReplClient::new())),
            documents: Arc::new(RwLock::new(DocumentTracker::new())),
            extractor: Mutex::new(
                TreeSitterExtractor::new().expect("tree-sitter common lisp grammar"),
            ),
            symbol_index,
            user_index,
            workspace_roots: Arc::new(RwLock::new(Vec::new())),
        }
    }

    async fn send_repl(&self, request: ReplRequest) -> anyhow::Result<common_rust::ReplResponse> {
        let client = Arc::clone(&self.master_repl);
        tokio::task::spawn_blocking(move || client.blocking_write().send_request(request))
            .await
            .map_err(|e| anyhow::anyhow!("master REPL task failed: {e}"))?
    }

    async fn notify_repl_file(&self, uri: &Url) {
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        let _ = self
            .send_repl(ReplRequest::SetCurrentFile {
                id: String::new(),
                path: path.to_string_lossy().into_owned(),
                contents: None,
            })
            .await;
    }

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
            debug!("No workspace roots configured, accepting all files (file: {})", uri);
            return true;
        }

        let uri_str = uri.as_str();

        for root in workspace_roots.iter() {
            if uri_in_workspace_root(uri_str, root.as_str()) {
                return true;
            }
        }

        debug!("File {} is outside workspace roots: {:?}", uri, *workspace_roots);
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

/// Convert a file byte/char offset to LSP Position without loading the rest of the file.
fn offset_to_position_in_file(path: &std::path::Path, offset: usize) -> Option<Position> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut buf = [0u8; 4096];
    let mut read = 0usize;
    let mut line = 0u32;
    let mut character = 0u32;
    while read < offset {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n.min(offset - read)] {
            if b == b'\n' {
                line += 1;
                character = 0;
            } else {
                character += 1;
            }
            read += 1;
        }
    }
    Some(Position { line, character })
}

fn workspace_root_uri(uri: Url) -> Url {
    let Ok(path) = uri.to_file_path() else {
        return uri;
    };
    if path.is_dir() {
        return uri;
    }
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            Url::from_file_path(parent).unwrap_or(uri)
        }
        _ => uri,
    }
}

fn uri_in_workspace_root(uri: &str, root: &str) -> bool {
    if uri == root {
        return true;
    }
    let root = root.trim_end_matches('/');
    uri.starts_with(root) && uri[root.len()..].starts_with('/')
}

fn is_system_lisp_path(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/sbcl/") || s.contains("\\sbcl\\") || s.contains("/lib/sbcl")
}

fn location_in_source_file(path: &str, symbol_name: &str) -> Option<Location> {
    let path = std::path::Path::new(path);
    if is_system_lisp_path(path) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut extractor = TreeSitterExtractor::new().ok()?;
    let pos = extractor
        .find_definitions(&text)
        .into_iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(symbol_name))
        .map(|(_, p)| p)?;
    let uri = Url::from_file_path(path).ok()?;
    Some(Location {
        uri,
        range: Range {
            start: pos,
            end: pos,
        },
    })
}

fn indexed_location(path: std::path::PathBuf, line: u32, character: u32) -> Option<Location> {
    let uri = Url::from_file_path(&path).ok()?;
    Some(Location {
        uri,
        range: Range {
            start: Position { line, character },
            end: Position { line, character },
        },
    })
}

impl LispLspBackend {
    fn create_external_location(
        &self,
        source_file: &str,
        source_line: Option<u32>,
        source_character: Option<u32>,
    ) -> Option<Location> {
        let path = std::path::Path::new(source_file);
        let uri = Url::from_file_path(path).ok()?;

        if let Some(line) = source_line.filter(|l| *l > 0) {
            let pos = Position {
                line: line.saturating_sub(1),
                character: 0,
            };
            return Some(Location {
                uri,
                range: Range {
                    start: pos,
                    end: pos,
                },
            });
        }

        if let Some(char_offset) = source_character.filter(|c| *c > 0) {
            let pos = offset_to_position_in_file(path, char_offset as usize)?;
            return Some(Location {
                uri,
                range: Range {
                    start: pos,
                    end: pos,
                },
            });
        }

        None
    }

    fn hover_from_buffer(text: &str, position: Position, symbol_name: &str) -> Option<Hover> {
        let mut extractor = TreeSitterExtractor::new().ok()?;
        let form = extractor.enclosing_form(text, position)?;
        let form = form.trim();
        if form.is_empty() {
            return None;
        }
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!(
                    "**{}** _buffer_\n\nNot loaded in the REPL. Select this whole form, then `repl: run` (Ctrl+Shift+Enter).\n\n```lisp\n{}\n```\n",
                    symbol_name,
                    form
                ),
            }),
            range: None,
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
            *workspace_roots = roots
                .into_iter()
                .map(|folder| workspace_root_uri(folder.uri))
                .collect();
            info!("Workspace roots: {:?}", *workspace_roots);
        } else if let Some(root_uri) = params.root_uri {
            let mut workspace_roots = self.workspace_roots.write().await;
            workspace_roots.push(workspace_root_uri(root_uri));
            info!("Workspace root: {:?}", workspace_roots.last());
        }

        info!("Workspace ready; starting master REPL in background");

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
        let client = Arc::clone(&self.master_repl);
        tokio::task::spawn_blocking(move || {
            client.blocking_write().try_start_master_repl();
        });
        self.client
            .log_message(MessageType::INFO, "Common Lisp LSP server ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down LSP server");
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

        self.notify_repl_file(&uri).await;

        // Only index files within workspace (skip external files like SBCL sources)
        if !self.is_workspace_file(&uri).await {
            debug!("Skipping index for file outside workspace: {}", uri);
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

        self.notify_repl_file(&uri).await;

        // Only index files within workspace (skip external files like SBCL sources)
        if !self.is_workspace_file(&uri).await {
            debug!("Skipping index for file outside workspace: {}", uri);
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
        self.notify_repl_file(&uri).await;

        debug!("Hover request at {}:{}:{}", uri, position.line, position.character);

        let (text, extraction_result) = {
            let documents = self.documents.read().await;
            let text = match documents.get(&uri) {
                Some(text) => text.clone(),
                None => return Ok(None),
            };
            let extraction_result = {
                let mut extractor = self.extractor.lock().await;
                extractor.symbol_with_package(&text, position)
            };
            (text, extraction_result)
        };

        let (symbol_name, package_name) = match extraction_result {
            Some((sym, pkg, _parents)) => (sym, pkg),
            None => {
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

        match self.send_repl(request).await {
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
                        Ok(Self::hover_from_buffer(&text, position, &symbol_name))
                    }
                    _ => {
                        debug!("Unexpected response type");
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                error!("Failed to query master REPL: {}", e);
                Ok(Self::hover_from_buffer(&text, position, &symbol_name))
            }
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        debug!("Completion request at {}:{}:{}", uri, position.line, position.character);

        // Get document text
        let (text, prefix) = {
            let documents = self.documents.read().await;
            let text = match documents.get(&uri) {
                Some(text) => text.clone(),
                None => return Ok(None),
            };
            let prefix = {
                let mut extractor = self.extractor.lock().await;
                match extractor.prefix_at_position(&text, position) {
                    Some(p) => p,
                    None => return Ok(None),
                }
            };
            (text, prefix)
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
            package: package_name.as_ref().map(|pkg| {
                if pkg.is_empty() {
                    "KEYWORD".to_string()
                } else {
                    pkg.clone()
                }
            }),
        };

        match self.send_repl(request).await {
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
                            if pkg.is_empty() {
                                symbols.into_iter()
                                    .filter(|info| info.package.eq_ignore_ascii_case("KEYWORD"))
                                    .collect()
                            } else {
                                symbols.into_iter()
                                    .filter(|info| info.package.eq_ignore_ascii_case(pkg))
                                    .collect()
                            }
                        } else {
                            symbols
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
        debug!("Definition request at {} {}", position.line, position.character);

        let (text, symbol_name, package_name) = {
            let documents = self.documents.read().await;
            let text = match documents.get(&uri) {
                Some(text) => text.clone(),
                None => return Ok(None),
            };
            let extracted = {
                let mut extractor = self.extractor.lock().await;
                extractor.symbol_with_package(&text, position)
            };
            match extracted {
                Some((sym, pkg, _)) => (text, sym, pkg),
                None => return Ok(None),
            }
        };

        if let Some(ref pkg) = package_name {
            let user_index = self.user_index.read().await;
            match user_index.lookup(pkg, &symbol_name) {
                Ok(Some((path, line, character))) => {
                    if let Some(location) = indexed_location(path, line, character) {
                        return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                    }
                }
                Ok(None) => {}
                Err(e) => error!("User index lookup error: {}", e),
            }
        }
        {
            let user_index = self.user_index.read().await;
            match user_index.lookup_symbol(&symbol_name) {
                Ok(Some((path, line, character))) => {
                    if let Some(location) = indexed_location(path, line, character) {
                        return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                    }
                }
                Ok(None) => {}
                Err(e) => error!("User index lookup error: {}", e),
            }
        }

        let packages_to_try: Vec<&str> = if let Some(ref pkg) = package_name {
            vec![pkg.as_str()]
        } else {
            vec!["COMMON-LISP", "SB-IMPL"]
        };

        for pkg in packages_to_try {
            let index = self.symbol_index.read().await;
            match index.lookup(pkg, &symbol_name) {
                Ok(Some(location)) => {
                    return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                }
                Ok(None) => {}
                Err(e) => error!("SBCL index lookup error: {}", e),
            }
        }

        let local_pos = {
            let mut extractor = self.extractor.lock().await;
            extractor
                .find_definitions(&text)
                .into_iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&symbol_name))
                .map(|(_, pos)| pos)
        };
        if let Some(local_pos) = local_pos {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: Range {
                    start: local_pos,
                    end: local_pos,
                },
            })));
        }

        let request = ReplRequest::SymbolInfo {
            id: String::new(),
            symbol: symbol_name.clone(),
            package: package_name,
        };

        match self.send_repl(request).await {
            Ok(response) => {
                use common_rust::ResponseData;
                match response.data {
                    ResponseData::SymbolInfo(info) => {
                        if let Some(ref source_file) = info.source_file {
                            if !source_file.is_empty() {
                                if let Some(location) = self.create_external_location(
                                    source_file,
                                    info.source_line,
                                    info.source_character,
                                ) {
                                    return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                                }
                                if let Some(location) =
                                    location_in_source_file(source_file, &symbol_name)
                                {
                                    return Ok(Some(GotoDefinitionResponse::Scalar(location)));
                                }
                            }
                        }
                        Ok(None)
                    }
                    ResponseData::Error { error } => {
                        error!("Master REPL returned error: {}", error);
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            }
            Err(e) => {
                error!("Failed to query master REPL: {}", e);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_file_uri_uses_parent_directory() {
        let dir = std::env::temp_dir();
        let file = dir.join("zed-cl-ws-root-test.lisp");
        std::fs::write(&file, "()").unwrap();
        let uri = Url::from_file_path(&file).unwrap();
        let root = workspace_root_uri(uri);
        assert_eq!(root.to_file_path().unwrap(), dir);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn workspace_match_requires_path_separator() {
        assert!(uri_in_workspace_root(
            "file:///tmp/examples/foo.lisp",
            "file:///tmp/examples"
        ));
        assert!(!uri_in_workspace_root(
            "file:///tmp/examples-extra/foo.lisp",
            "file:///tmp/examples"
        ));
    }

    #[test]
    fn location_in_source_file_finds_defun() {
        let dir = std::env::temp_dir();
        let file = dir.join("zed-cl-goto-test.lisp");
        std::fs::write(&file, ";;; header\n(defun multiply-numbers (a b)\n  (* a b))\n").unwrap();
        let loc = location_in_source_file(file.to_str().unwrap(), "multiply-numbers").unwrap();
        assert_eq!(loc.range.start.line, 1);
        let _ = std::fs::remove_file(&file);
    }
}
