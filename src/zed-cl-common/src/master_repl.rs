/// Client for communicating with the master REPL via Unix sockets
/// Supports multiple Common Lisp implementations (SBCL, ECL)

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::protocol::{ReplRequest, ReplResponse, ResponseData, SymbolInfo};

const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Get implementation-specific socket path
/// Returns `/tmp/zed-cl-repl-{impl}.sock` where {impl} is from config
pub fn get_socket_path() -> PathBuf {
    crate::config::Config::get().socket_path()
}

// Helper types for deserializing s-expressions
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct SexpSymbolInfo {
    #[serde(rename = ":symbol", alias = ":SYMBOL")]
    symbol: String,
    #[serde(rename = ":package", alias = ":PACKAGE")]
    package: String,
    #[serde(rename = ":kind", alias = ":KIND")]
    kind: String,
    #[serde(rename = ":source", alias = ":SOURCE", default)]
    source: Option<String>,
    #[serde(rename = ":doc", alias = ":DOC", default)]
    doc: Option<String>,
}

/// Client for the master REPL via Unix socket (synchronous)
pub struct MasterReplClient {
    socket_path: PathBuf,
    stream: Option<UnixStream>,
    request_counter: u64,
    extension_dir: Option<PathBuf>,
    auto_restart_enabled: bool,
    repl_starting: Arc<AtomicBool>,
}

impl MasterReplClient {
    /// Create a new master REPL client
    pub fn new(endpoint: impl Into<String>) -> Self {
        // Endpoint parameter ignored - kept for API compatibility
        let _ = endpoint.into();

        // Try to get extension directory from environment
        let extension_dir = std::env::var("ZED_CL_EXTENSION_DIR")
            .ok()
            .map(PathBuf::from);

        Self {
            socket_path: get_socket_path(),
            stream: None,
            request_counter: 0,
            extension_dir,
            auto_restart_enabled: true,
            repl_starting: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Expand ~ in paths to home directory
    fn expand_tilde(path: &PathBuf) -> PathBuf {
        if let Some(path_str) = path.to_str() {
            if path_str.starts_with("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    return PathBuf::from(home).join(&path_str[2..]);
                }
            } else if path_str == "~" {
                if let Ok(home) = std::env::var("HOME") {
                    return PathBuf::from(home);
                }
            }
        }
        path.clone()
    }

    /// Check if master REPL process is running by trying to connect
    fn is_master_repl_alive(&self) -> bool {
        UnixStream::connect(&self.socket_path).is_ok()
    }

    /// Attempt to start the master REPL if not running
    /// Returns immediately after spawning, doesn't wait for REPL to be ready
    pub fn try_start_master_repl(&self) {
        // Check if already starting (atomic compare-and-swap)
        if self.repl_starting.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            debug!("Master REPL already starting, skipping duplicate spawn");
            return;
        }

        if !self.auto_restart_enabled {
            warn!("Auto-restart is disabled");
            self.repl_starting.store(false, Ordering::SeqCst);
            return;
        }

        // Check if already running
        if self.is_master_repl_alive() {
            debug!("Master REPL already running");
            self.repl_starting.store(false, Ordering::SeqCst);
            return;
        }

        info!("Master REPL not running, starting in background...");

        // Need extension directory to find startup script
        let ext_dir = match self.extension_dir.as_ref() {
            Some(dir) => dir,
            None => {
                error!("ZED_CL_EXTENSION_DIR not set, cannot auto-start master REPL");
                self.repl_starting.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Expand ~ in extension directory path
        let ext_dir = Self::expand_tilde(ext_dir);

        // Get Lisp implementation from config
        let config = crate::config::Config::get();
        let lisp_impl = &config.lisp_impl;

        let (lisp_cmd, lisp_args) = match lisp_impl.as_str() {
            "sbcl" => {
                match which::which("sbcl") {
                    Ok(path) => (path, vec!["--noinform", "--no-userinit", "--load"]),
                    Err(_) => {
                        error!("SBCL not found in PATH. Please install: brew install sbcl");
                        error!("Note: LSP features like hover and goto-definition will still work for indexed code");
                        self.repl_starting.store(false, Ordering::SeqCst);
                        return;
                    }
                }
            }
            "ecl" => {
                match which::which("ecl") {
                    Ok(path) => (path, vec!["-norc", "-load"]),
                    Err(_) => {
                        error!("ECL not found in PATH (ZED_CL_LISP_IMPL=ecl). Please install: brew install ecl");
                        error!("Note: LSP features like hover and goto-definition will still work for indexed code");
                        self.repl_starting.store(false, Ordering::SeqCst);
                        return;
                    }
                }
            }
            other => {
                error!("Unsupported Lisp implementation: {}. Supported: sbcl, ecl", other);
                error!("Note: LSP features like hover and goto-definition will still work for indexed code");
                self.repl_starting.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Path to startup script
        let start_script = ext_dir.join("repl").join("start-master-repl.lisp");
        if !start_script.exists() {
            error!("Master REPL startup script not found at {:?}", start_script);
            self.repl_starting.store(false, Ordering::SeqCst);
            return;
        }

        // Create log file
        let log_file = match std::fs::File::create("/tmp/master-repl.log") {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create master REPL log file: {}", e);
                self.repl_starting.store(false, Ordering::SeqCst);
                return;
            }
        };

        let log_file_err = match log_file.try_clone() {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to clone log file handle: {}", e);
                self.repl_starting.store(false, Ordering::SeqCst);
                return;
            }
        };

        info!("Spawning master REPL in background with {:?}...", lisp_cmd);

        // Start master REPL as detached process
        let mut cmd = Command::new(&lisp_cmd);
        for arg in lisp_args {
            cmd.arg(arg);
        }

        match cmd
            .arg(&start_script)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_file_err))
            .spawn()
        {
            Ok(child) => {
                info!("Master REPL spawned with PID: {} (will be ready in ~100ms)", child.id());
                self.repl_starting.store(false, Ordering::SeqCst);
            }
            Err(e) => {
                error!("Failed to spawn master REPL: {}", e);
                self.repl_starting.store(false, Ordering::SeqCst);
            }
        }
    }

    /// Ensure we have a connected stream
    fn ensure_connected(&mut self) -> Result<()> {
        // If we already have a stream, return
        if self.stream.is_some() {
            return Ok(());
        }

        // Try to connect
        match UnixStream::connect(&self.socket_path) {
            Ok(stream) => {
                stream.set_read_timeout(Some(READ_TIMEOUT))?;
                stream.set_write_timeout(Some(READ_TIMEOUT))?;
                debug!("Connected to master REPL socket");
                self.stream = Some(stream);
                Ok(())
            }
            Err(_) => {
                // Not running, try to start
                self.try_start_master_repl();

                // Wait a bit for startup
                std::thread::sleep(Duration::from_millis(200));

                // Try to connect again
                let stream = UnixStream::connect(&self.socket_path)
                    .context("Failed to connect to master REPL after starting")?;

                stream.set_read_timeout(Some(READ_TIMEOUT))?;
                stream.set_write_timeout(Some(READ_TIMEOUT))?;

                self.stream = Some(stream);
                Ok(())
            }
        }
    }

    /// Connect to the master REPL
    /// Tests that REPL is actually alive and responding
    pub fn connect(&mut self) -> Result<()> {
        debug!("Connecting to master REPL at {:?}", self.socket_path);
        self.ensure_connected()?;
        debug!("Connected to master REPL");
        Ok(())
    }

    /// Check if connected to master REPL
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Generate a unique request ID
    fn next_request_id(&mut self) -> String {
        self.request_counter += 1;
        format!("rust-{}", self.request_counter)
    }

    /// Send a request to the master REPL and wait for response
    pub fn send_request(&mut self, mut request: ReplRequest) -> Result<ReplResponse> {
        // Ensure we're connected
        self.ensure_connected()?;

        // Set request ID if needed
        match &mut request {
            ReplRequest::SymbolInfo { id, .. }
            | ReplRequest::ListSymbols { id, .. }
            | ReplRequest::Eval { id, .. }
            | ReplRequest::LoadFile { id, .. }
            | ReplRequest::SetCurrentFile { id, .. } => {
                if id.is_empty() {
                    *id = self.next_request_id();
                }
            }
        }

        let request_id = match &request {
            ReplRequest::SymbolInfo { id, .. }
            | ReplRequest::ListSymbols { id, .. }
            | ReplRequest::Eval { id, .. }
            | ReplRequest::LoadFile { id, .. }
            | ReplRequest::SetCurrentFile { id, .. } => id.clone(),
        };

        // Convert to s-expression
        let sexp = request.to_sexp();
        debug!("Sending request: {}", sexp);

        // Get the stream
        let stream = self.stream.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        // Send request (with newline)
        stream.write_all(sexp.as_bytes())
            .context("Failed to send request")?;
        stream.write_all(b"\n")
            .context("Failed to send newline")?;
        stream.flush()
            .context("Failed to flush stream")?;

        debug!("Request sent, waiting for response...");

        // Read complete s-expression response
        // S-expressions can span multiple lines, so we need to read until balanced parens
        let mut response = String::new();
        let mut byte_buf = [0u8; 1];
        let mut paren_depth = 0;
        let mut in_string = false;
        let mut escape_next = false;
        let mut started = false;

        loop {
            match stream.read_exact(&mut byte_buf) {
                Ok(_) => {
                    let ch = byte_buf[0] as char;
                    response.push(ch);

                    if escape_next {
                        escape_next = false;
                        continue;
                    }

                    match ch {
                        '\\' if in_string => {
                            escape_next = true;
                        }
                        '"' => {
                            in_string = !in_string;
                        }
                        '(' if !in_string => {
                            paren_depth += 1;
                            started = true;
                        }
                        ')' if !in_string => {
                            paren_depth -= 1;
                            // Complete s-expression when parens are balanced
                            if started && paren_depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    self.stream = None;
                    return Err(e).context("Failed to read response");
                }
            }
        }

        if response.is_empty() {
            self.stream = None;
            anyhow::bail!("Connection closed by master REPL");
        }

        debug!("Received response: {}", response.trim());
        self.parse_response(&response, &request_id)
    }

    /// Parse s-expression response from master REPL
    ///
    /// The master REPL sends responses as property lists (plists).
    /// Example: (:ID "123" :SYMBOL "DEFUN" :KIND "function" :PACKAGE "CL-USER")
    fn parse_response(&self, sexp: &str, expected_id: &str) -> Result<ReplResponse> {
        info!("Parsing s-expression: {}", sexp);

        // Convert plist to HashMap for easier processing
        let map = self.sexp_to_plist_map(sexp)?;

        // Extract ID
        let id = map.get("ID")
            .or_else(|| map.get("id"))
            .cloned()
            .unwrap_or_else(|| expected_id.to_string());

        // Determine response type by checking which fields are present
        if let Some(error) = map.get("ERROR").or_else(|| map.get("error")) {
            // Error response
            Ok(ReplResponse {
                id,
                data: ResponseData::Error {
                    error: error.clone(),
                },
            })
        } else if map.contains_key("SYMBOL") || map.contains_key("symbol") {
            // SymbolInfo response - extract from map
            let symbol = map.get("SYMBOL").or_else(|| map.get("symbol"))
                .cloned().unwrap_or_default();
            let package = map.get("PACKAGE").or_else(|| map.get("package"))
                .cloned().unwrap_or_default();
            let kind = map.get("KIND").or_else(|| map.get("kind"))
                .cloned().unwrap_or_default();
            let source = map.get("SOURCE").or_else(|| map.get("source"))
                .cloned();
            let doc = map.get("DOC").or_else(|| map.get("doc"))
                .cloned();
            let source_file = map.get("SOURCE-FILE").or_else(|| map.get("source-file"))
                .cloned();
            let source_line = map.get("SOURCE-LINE").or_else(|| map.get("source-line"))
                .and_then(|s| s.parse::<u32>().ok());
            let source_character = map.get("SOURCE-CHARACTER").or_else(|| map.get("source-character"))
                .and_then(|s| s.parse::<u32>().ok());

            Ok(ReplResponse {
                id,
                data: ResponseData::SymbolInfo(SymbolInfo {
                    symbol,
                    package,
                    kind,
                    source,
                    doc,
                    param_types: None, // TODO: Parse PARAM-TYPES from sexp
                    source_file,
                    source_line,
                    source_character,
                }),
            })
        } else if map.contains_key("SYMBOLS") || map.contains_key("symbols") {
            // SymbolList response - parse the symbols list
            let symbols = self.extract_symbol_list(sexp)?;

            Ok(ReplResponse {
                id,
                data: ResponseData::SymbolList { symbols },
            })
        } else if map.contains_key("OUTPUT") || map.contains_key("output") {
            // EvalResult response
            let output = map.get("OUTPUT")
                .or_else(|| map.get("output"))
                .cloned()
                .unwrap_or_default();

            let values = map.get("VALUES")
                .or_else(|| map.get("values"))
                .and_then(|v| self.parse_value_as_string_list(v))
                .unwrap_or_default();

            let error = map.get("ERROR")
                .or_else(|| map.get("error"))
                .filter(|s| !s.is_empty())
                .cloned();

            let traceback = map.get("TRACEBACK")
                .or_else(|| map.get("traceback"))
                .filter(|s| !s.is_empty())
                .cloned();

            let displays = self.extract_displays(sexp);

            Ok(ReplResponse {
                id,
                data: ResponseData::EvalResult {
                    output,
                    values,
                    error,
                    traceback,
                    displays,
                },
            })
        } else {
            anyhow::bail!("Unknown response format: {:?}", map.keys());
        }
    }

    /// Convert s-expression string to a string map (flattened plist)
    fn sexp_to_plist_map(&self, sexp: &str) -> Result<HashMap<String, String>> {
        let mut map = HashMap::new();

        let s = sexp.trim();

        // Remove outer parens
        let s = if s.starts_with('(') && s.ends_with(')') {
            &s[1..s.len()-1]
        } else {
            s
        };

        // Simple tokenizer for key-value pairs
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_string = false;
        let mut escape = false;

        for ch in s.chars() {
            if escape {
                current.push(ch);
                escape = false;
            } else if ch == '\\' {
                escape = true;
                current.push(ch);
            } else if ch == '"' {
                in_string = !in_string;
                current.push(ch);
            } else if ch.is_whitespace() && !in_string {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            } else {
                current.push(ch);
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }

        // Parse key-value pairs
        let mut i = 0;
        while i < tokens.len() - 1 {
            let key = tokens[i].trim_start_matches(':').to_uppercase();
            i += 1;

            // Check if value is NIL (Lisp's null)
            if tokens[i].to_uppercase() == "NIL" {
                // Skip NIL values - treat as absent
                i += 1;
                continue;
            }

            // Collect value (might be multiple tokens if it's a list)
            let mut value = tokens[i].trim_matches('"').to_string();

            // If value starts with (, collect until matching )
            if tokens[i].starts_with('(') {
                let mut depth = 0;
                let mut val_tokens = vec![tokens[i].clone()];
                for ch in tokens[i].chars() {
                    if ch == '(' { depth += 1; }
                    else if ch == ')' { depth -= 1; }
                }

                i += 1;
                while depth > 0 && i < tokens.len() {
                    val_tokens.push(tokens[i].clone());
                    for ch in tokens[i].chars() {
                        if ch == '(' { depth += 1; }
                        else if ch == ')' { depth -= 1; }
                    }
                    i += 1;
                }
                value = val_tokens.join(" ");
            } else {
                i += 1;
            }

            map.insert(key, value);
        }

        Ok(map)
    }

    /// Parse a value as a list of strings
    fn parse_value_as_string_list(&self, s: &str) -> Option<Vec<String>> {
        // Remove parens and split
        let s = s.trim();
        let s = if s.starts_with('(') && s.ends_with(')') {
            &s[1..s.len()-1]
        } else {
            return None;
        };

        if s.trim().is_empty() || s.trim() == "NIL" {
            return Some(Vec::new());
        }

        // Parse quoted strings
        let mut result = Vec::new();
        let mut current = String::new();
        let mut in_string = false;
        let mut escape = false;

        for ch in s.chars() {
            if escape {
                current.push(ch);
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                if in_string {
                    result.push(current.clone());
                    current.clear();
                }
                in_string = !in_string;
            } else if in_string {
                current.push(ch);
            }
        }

        Some(result)
    }

    /// Extract symbol list from s-expression
    fn extract_symbol_list(&self, sexp: &str) -> Result<Vec<SymbolInfo>> {
        // Parse the response to extract the :SYMBOLS list
        // Format: (:ID "..." :SYMBOLS ((:SYMBOL "..." :PACKAGE "..." :KIND "..." ...) ...))

        // Find the :SYMBOLS keyword and extract the list after it
        let symbols_start = if let Some(pos) = sexp.find(":SYMBOLS") {
            pos + ":SYMBOLS".len()
        } else if let Some(pos) = sexp.find(":symbols") {
            pos + ":symbols".len()
        } else {
            return Ok(Vec::new());
        };

        // Find the opening paren of the symbols list
        let rest = &sexp[symbols_start..];
        let list_start = if let Some(pos) = rest.find('(') {
            symbols_start + pos
        } else {
            return Ok(Vec::new());
        };

        // Extract each symbol plist from the list
        let mut symbols = Vec::new();
        let mut depth = 0;
        let mut current_symbol = String::new();
        let mut in_symbol = false;

        for ch in sexp[list_start..].chars() {
            if ch == '(' {
                depth += 1;
                if depth == 2 {
                    // Start of a symbol plist
                    in_symbol = true;
                    current_symbol.clear();
                }
                if in_symbol {
                    current_symbol.push(ch);
                }
            } else if ch == ')' {
                if in_symbol {
                    current_symbol.push(ch);
                }
                depth -= 1;
                if depth == 1 && in_symbol {
                    // End of a symbol plist
                    in_symbol = false;
                    // Parse this symbol
                    if let Ok(map) = self.sexp_to_plist_map(&current_symbol) {
                        let symbol = map.get("SYMBOL").or_else(|| map.get("symbol"))
                            .cloned().unwrap_or_default();
                        let package = map.get("PACKAGE").or_else(|| map.get("package"))
                            .cloned().unwrap_or_default();
                        let kind = map.get("KIND").or_else(|| map.get("kind"))
                            .cloned().unwrap_or_default();
                        let source = map.get("SOURCE").or_else(|| map.get("source"))
                            .cloned();
                        let doc = map.get("DOC").or_else(|| map.get("doc"))
                            .cloned();

                        // Parse param-types if available: (("NAME" . "STRING") ("AGE" . "FIXNUM"))
                        let param_types = map.get("PARAM-TYPES").or_else(|| map.get("param-types"))
                            .and_then(|pt_str| self.parse_param_types(pt_str).ok());

                        let source_file = map.get("SOURCE-FILE").or_else(|| map.get("source-file"))
                            .cloned();
                        let source_line = map.get("SOURCE-LINE").or_else(|| map.get("source-line"))
                            .and_then(|s| s.parse::<u32>().ok());
                        let source_character = map.get("SOURCE-CHARACTER").or_else(|| map.get("source-character"))
                            .and_then(|s| s.parse::<u32>().ok());

                        symbols.push(SymbolInfo {
                            symbol,
                            package,
                            kind,
                            source,
                            doc,
                            param_types,
                            source_file,
                            source_line,
                            source_character,
                        });
                    }
                }
                if depth == 0 {
                    break;
                }
            } else if in_symbol {
                current_symbol.push(ch);
            }
        }

        Ok(symbols)
    }

    /// Extract displays list from response
    /// Format: (:DISPLAYS ((:DATA (("text/html" . "<h1>...</h1>")) :METADATA NIL) ...))
    fn extract_displays(&self, sexp: &str) -> Option<Vec<crate::protocol::DisplayData>> {
        // Find the :DISPLAYS keyword
        let displays_start = if let Some(pos) = sexp.find(":DISPLAYS") {
            info!("Found :DISPLAYS at position {}", pos);
            pos + ":DISPLAYS".len()
        } else if let Some(pos) = sexp.find(":displays") {
            info!("Found :displays at position {}", pos);
            pos + ":displays".len()
        } else {
            info!("No :DISPLAYS or :displays found in response");
            return None;
        };

        // Find the opening paren of the displays list
        let rest = &sexp[displays_start..];
        let list_start = rest.find('(')?;

        let mut displays = Vec::new();
        let mut depth = 0;
        let mut current_display = String::new();
        let mut in_display = false;

        for ch in rest[list_start..].chars() {
            if ch == '(' {
                depth += 1;
                if depth == 2 {
                    // Start of a display plist
                    in_display = true;
                    current_display.clear();
                }
                if in_display {
                    current_display.push(ch);
                }
            } else if ch == ')' {
                if in_display {
                    current_display.push(ch);
                }
                depth -= 1;
                if depth == 1 && in_display {
                    // End of a display plist
                    in_display = false;

                    // Parse this display: (:DATA ((...)) :METADATA ...)
                    info!("Parsing display item: {}", &current_display[..current_display.len().min(100)]);
                    match self.parse_display_item(&current_display) {
                        Ok(display_data) => {
                            info!("Successfully parsed display with {} MIME types", display_data.data.len());
                            displays.push(display_data);
                        }
                        Err(e) => {
                            warn!("Failed to parse display item: {}", e);
                        }
                    }
                }
                if depth == 0 {
                    break;
                }
            } else if in_display {
                current_display.push(ch);
            }
        }

        if displays.is_empty() {
            None
        } else {
            Some(displays)
        }
    }

    /// Parse a single display item from plist
    fn parse_display_item(&self, sexp: &str) -> Result<crate::protocol::DisplayData> {
        use std::collections::HashMap;

        // Extract :DATA alist
        let data = if let Some(data_start) = sexp.find(":DATA").or_else(|| sexp.find(":data")) {
            let after_data = &sexp[data_start + 5..];
            if let Some(list_start) = after_data.find('(') {
                self.parse_alist(&after_data[list_start..])?
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        Ok(crate::protocol::DisplayData {
            data,
            metadata: None, // TODO: Parse metadata if needed
        })
    }

    /// Unescape a Lisp string (remove backslash escapes)
    fn unescape_lisp_string(&self, s: &str) -> String {
        let mut result = String::new();
        let mut chars = s.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                // Skip backslash, take next char literally
                if let Some(next_ch) = chars.next() {
                    result.push(next_ch);
                }
            } else {
                result.push(ch);
            }
        }
        result
    }

    /// Parse an alist like (("key1" . "value1") ("key2" . "value2"))
    fn parse_alist(&self, sexp: &str) -> Result<std::collections::HashMap<String, String>> {
        use std::collections::HashMap;
        let mut map = HashMap::new();

        let trimmed = sexp.trim();
        if !trimmed.starts_with('(') {
            return Ok(map);
        }

        let mut depth = 0;
        let mut current_pair = String::new();
        let mut in_pair = false;

        for ch in trimmed.chars() {
            if ch == '(' {
                depth += 1;
                if depth == 2 {
                    // Start of a pair - don't include the opening paren
                    in_pair = true;
                    current_pair.clear();
                }
                // Don't push the opening paren of the pair itself
                else if in_pair && depth > 2 {
                    current_pair.push(ch);
                }
            } else if ch == ')' {
                if in_pair && depth > 2 {
                    current_pair.push(ch);
                }
                depth -= 1;
                if depth == 1 && in_pair {
                    // End of a pair
                    in_pair = false;

                    // Parse KEY . VALUE (without outer parens)
                    if let Some(dot_pos) = current_pair.find(" . ") {
                        let key = current_pair[..dot_pos].trim().trim_matches('"').to_string();
                        let value_raw = current_pair[dot_pos + 3..].trim().trim_matches('"').to_string();
                        // Unescape backslash sequences in the value
                        let value = self.unescape_lisp_string(&value_raw);
                        if value != "NIL" {
                            map.insert(key, value);
                        }
                    }
                }
                if depth == 0 {
                    break;
                }
            } else if in_pair {
                current_pair.push(ch);
            }
        }

        Ok(map)
    }

    /// Parse param-types from s-expression
    /// Example: (("NAME" . "STRING") ("AGE" . "FIXNUM")) or (("NAME" . NIL))
    fn parse_param_types(&self, sexp: &str) -> Result<Vec<(String, Option<String>)>> {
        let trimmed = sexp.trim();

        // Should be a list of cons cells: ((NAME . TYPE) ...)
        if !trimmed.starts_with('(') || !trimmed.ends_with(')') {
            return Ok(Vec::new());
        }

        let mut result = Vec::new();
        let content = &trimmed[1..trimmed.len() - 1].trim();

        // Simple parser for (("NAME" . "TYPE") ("NAME2" . "TYPE2"))
        let mut depth = 0;
        let mut current_pair = String::new();

        for ch in content.chars() {
            if ch == '(' {
                depth += 1;
                if depth == 1 {
                    current_pair.clear();
                }
            } else if ch == ')' {
                depth -= 1;
                if depth == 0 && !current_pair.is_empty() {
                    // Parse the pair: "NAME" . "TYPE"
                    if let Some(dot_pos) = current_pair.find('.') {
                        let name_part = current_pair[..dot_pos].trim();
                        let type_part = current_pair[dot_pos + 1..].trim();

                        // Remove quotes if present
                        let name = name_part.trim_matches('"').to_string();
                        let type_val = if type_part == "NIL" || type_part.is_empty() {
                            None
                        } else {
                            Some(type_part.trim_matches('"').to_string())
                        };
                        result.push((name, type_val));
                    }
                    current_pair.clear();
                }
            } else if depth > 0 {
                current_pair.push(ch);
            }
        }

        Ok(result)
    }

    /// Close the connection
    pub fn close(&mut self) -> Result<()> {
        debug!("Closing Unix socket connection");
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
        Ok(())
    }
}
