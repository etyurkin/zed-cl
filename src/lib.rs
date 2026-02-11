use zed_extension_api as zed;
use zed::settings::LspSettings;
use std::path::PathBuf;
use std::fs;
use std::io::Write;

// Include binaries at compile time
const LSP_BINARY: &[u8] = include_bytes!("../bin/zed-cl-lsp");
const KERNEL_BINARY: &[u8] = include_bytes!("../bin/zed-cl-kernel");
const INDEXER_BINARY: &[u8] = include_bytes!("../bin/zed-cl-index");

// Include kernel.json template
const KERNEL_JSON_TEMPLATE: &str = include_str!("../kernels/commonlisp/kernel.json");

// Include all REPL files from src/zed-cl-repl-impl/ directory
const ZED_CL_ASD: &str = include_str!("zed-cl-repl-impl/zed-cl.asd");
const START_MASTER_REPL: &str = include_str!("zed-cl-repl-impl/start-master-repl.lisp");
const BOOTSTRAP: &str = include_str!("zed-cl-repl-impl/bootstrap.lisp");
const COMPAT: &str = include_str!("zed-cl-repl-impl/compat.lisp");
const CONFIG: &str = include_str!("zed-cl-repl-impl/config.lisp");
const DISPLAY: &str = include_str!("zed-cl-repl-impl/display.lisp");
const SOCKET_SERVER: &str = include_str!("zed-cl-repl-impl/socket-server.lisp");
const MASTER_REPL: &str = include_str!("zed-cl-repl-impl/master-repl.lisp");

struct CommonLispExtension {
    extension_dir: Option<PathBuf>,
}

impl zed::Extension for CommonLispExtension {
    fn new() -> Self {
        // Get extension directory from PWD environment variable
        // Zed sets PWD to the installed extension location
        let extension_dir = std::env::var("PWD")
            .ok()
            .map(|pwd| {
                // Expand ~ if present (convert to absolute path)
                if pwd.starts_with("~/") {
                    if let Ok(home) = std::env::var("HOME") {
                        PathBuf::from(home).join(&pwd[2..])
                    } else {
                        PathBuf::from(pwd)
                    }
                } else if pwd == "~" {
                    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(pwd))
                } else {
                    PathBuf::from(pwd)
                }
            });

        Self { extension_dir }
    }

    fn label_for_completion(
        &self,
        _language_server_id: &zed::LanguageServerId,
        completion: zed::lsp::Completion,
    ) -> Option<zed::CodeLabel> {
        use zed::lsp::CompletionKind;
        use zed::CodeLabelSpan;

        // Label format: "[package] symbol-name"
        // Detail format: "function (a b)" or "variable"
        let label = &completion.label;
        let detail = completion.detail.as_ref().map(|s| s.as_str()).unwrap_or("");

        // Parse label to extract package and name
        // Formats: "[package] symbol" or "[package]" for package completions
        let (package, name) = if label.starts_with('[') {
            // Has package: "[package] symbol" or just "[package]"
            if let Some(close_bracket) = label.find(']') {
                let pkg = &label[1..close_bracket];
                let rest_start = close_bracket + 1;
                let rest = if rest_start < label.len() {
                    label[rest_start..].trim()
                } else {
                    ""
                };
                (pkg, rest)
            } else {
                ("", label.as_str())
            }
        } else {
            ("", label.as_str())
        };

        // Build code string with manual syntax highlighting
        let mut code = String::new();
        let mut spans = vec![];

        // Package in brackets (shown first, only if not empty)
        if !package.is_empty() {
            code.push('[');
            spans.push(CodeLabelSpan::literal("[", None));
            code.push_str(package);
            spans.push(CodeLabelSpan::literal(package, Some("constant".to_string())));
            code.push(']');
            spans.push(CodeLabelSpan::literal("]", None));

            // Add space after bracket only if there's a name to follow
            if !name.is_empty() {
                code.push(' ');
                spans.push(CodeLabelSpan::literal(" ", None));
            }
        }

        // Symbol name - highlight based on completion kind
        if !name.is_empty() {
            code.push_str(name);
            let name_highlight = match completion.kind {
                Some(CompletionKind::Function) | Some(CompletionKind::Method) => Some("function".to_string()),
                Some(CompletionKind::Variable) | Some(CompletionKind::Constant) => Some("variable".to_string()),
                Some(CompletionKind::Module) => Some("constant".to_string()), // Yellow for packages
                _ => None,
            };
            spans.push(CodeLabelSpan::literal(name, name_highlight));
        }

        // Space before detail (only if there's detail to show)
        if !detail.is_empty() {
            code.push(' ');
            spans.push(CodeLabelSpan::literal(" ", None));
        }

        // Detail: parse "function (a b)" or "variable"
        // Color the kind and parentheses separately
        if let Some(paren_pos) = detail.find('(') {
            // Has parameters: "function (a b)"
            let kind = detail[..paren_pos].trim_end();
            let params_with_parens = &detail[paren_pos..];

            // Kind in keyword color (blue)
            code.push_str(kind);
            spans.push(CodeLabelSpan::literal(kind.to_string(), Some("keyword".to_string())));
            code.push(' ');
            spans.push(CodeLabelSpan::literal(" ", None));

            // Opening paren in constant color (yellow)
            code.push('(');
            spans.push(CodeLabelSpan::literal("(", Some("constant".to_string())));

            // Parameters in default color
            if let Some(close_paren_pos) = params_with_parens.find(')') {
                let params_inside = &params_with_parens[1..close_paren_pos];
                code.push_str(params_inside);
                spans.push(CodeLabelSpan::literal(params_inside.to_string(), None));

                // Closing paren in constant color (yellow)
                code.push(')');
                spans.push(CodeLabelSpan::literal(")", Some("constant".to_string())));
            }
        } else {
            // No parameters, just the kind
            code.push_str(detail);
            spans.push(CodeLabelSpan::literal(detail.to_string(), Some("keyword".to_string())));
        }

        // For filter_range, include the package name and symbol name
        // This allows matching on either symbol name or package name
        // Format: "[package] symbol-name"
        let filter_end = if !package.is_empty() {
            // Include "[package] symbol" in filter range
            1 + package.len() + 2 + name.len() // "[package] name"
        } else {
            name.len()
        };

        Some(zed::CodeLabel {
            code,
            spans,
            filter_range: (0..filter_end).into(),
        })
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command, String> {
        if language_server_id.as_ref() == "zed-cl" {
            // Get the work directory where we can write files
            // Use extension_dir from PWD if available, otherwise fall back to current_dir
            let work_dir = self.extension_dir.clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            // Create bin directory in work dir
            let bin_dir = work_dir.join("bin");
            fs::create_dir_all(&bin_dir)
                .map_err(|e| format!("Failed to create bin directory: {}", e))?;

            // Path to binaries
            let lsp_binary = bin_dir.join("zed-cl-lsp");
            let kernel_binary = bin_dir.join("zed-cl-kernel");
            let indexer_binary = bin_dir.join("zed-cl-index");

            // Write binaries (always overwrite to pick up updates)
            let mut file = fs::File::create(&lsp_binary)
                .map_err(|e| format!("Failed to create LSP binary: {}", e))?;
            file.write_all(LSP_BINARY)
                .map_err(|e| format!("Failed to write LSP binary: {}", e))?;

            let mut file = fs::File::create(&kernel_binary)
                .map_err(|e| format!("Failed to create kernel binary: {}", e))?;
            file.write_all(KERNEL_BINARY)
                .map_err(|e| format!("Failed to write kernel binary: {}", e))?;

            let mut file = fs::File::create(&indexer_binary)
                .map_err(|e| format!("Failed to create indexer binary: {}", e))?;
            file.write_all(INDEXER_BINARY)
                .map_err(|e| format!("Failed to write indexer binary: {}", e))?;

            // Make binaries executable (cross-platform)
            zed::make_file_executable(&lsp_binary.to_string_lossy())?;
            zed::make_file_executable(&kernel_binary.to_string_lossy())?;
            zed::make_file_executable(&indexer_binary.to_string_lossy())?;

            // Register Jupyter kernel for REPL functionality
            let jupyter_dir = if cfg!(target_os = "macos") {
                PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
                    .join("Library/Jupyter/kernels/commonlisp-zed")
            } else {
                PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
                    .join(".local/share/jupyter/kernels/commonlisp-zed")
            };

            fs::create_dir_all(&jupyter_dir)
                .map_err(|e| format!("Failed to create Jupyter kernel directory: {}", e))?;

            // Use kernel.json template and substitute the kernel path
            let kernel_json = KERNEL_JSON_TEMPLATE
                .replace("KERNEL_PATH_PLACEHOLDER", &kernel_binary.to_string_lossy());

            fs::write(jupyter_dir.join("kernel.json"), kernel_json)
                .map_err(|e| format!("Failed to write kernel.json: {}", e))?;

            // Create repl directory for all REPL source files
            let repl_dir = work_dir.join("repl");
            fs::create_dir_all(&repl_dir)
                .map_err(|e| format!("Failed to create repl directory: {}", e))?;

            // Write files (always overwrite to pick up updates during development)
            let check_and_write = |path: &std::path::Path, content: &str| -> Result<(), String> {
                fs::write(path, content)
                    .map_err(|e| format!("Failed to write {}: {}", path.display(), e))
            };

            // Write all REPL files to repl/ (including ASDF system)
            check_and_write(&repl_dir.join("zed-cl.asd"), ZED_CL_ASD)?;
            check_and_write(&repl_dir.join("start-master-repl.lisp"), START_MASTER_REPL)?;
            check_and_write(&repl_dir.join("bootstrap.lisp"), BOOTSTRAP)?;
            check_and_write(&repl_dir.join("compat.lisp"), COMPAT)?;
            check_and_write(&repl_dir.join("config.lisp"), CONFIG)?;
            check_and_write(&repl_dir.join("display.lisp"), DISPLAY)?;
            check_and_write(&repl_dir.join("socket-server.lisp"), SOCKET_SERVER)?;
            check_and_write(&repl_dir.join("master-repl.lisp"), MASTER_REPL)?;

            // Verify sbcl exists
            worktree
                .which("sbcl")
                .ok_or_else(|| "sbcl not found in PATH. Please install SBCL: https://www.sbcl.org/".to_string())?;

            // Read LSP settings for custom configuration
            let settings = LspSettings::for_worktree("zed-cl", worktree)
                .ok()
                .and_then(|s| s.settings);

            // Set socket path for master REPL communication
            let socket_path = settings
                .as_ref()
                .and_then(|s| s.get("master_repl_socket"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "/tmp/zed-cl-master-repl.sock".to_string());

            // Get system index name from settings
            let system_index_name = settings
                .as_ref()
                .and_then(|s| s.get("system_index"))
                .and_then(|v| v.as_str())
                .unwrap_or("system-index.db");

            // Build environment variables
            let mut env = vec![
                ("ZED_CL_MASTER_REPL_SOCKET".to_string(), socket_path),
                ("ZED_CL_EXTENSION_DIR".to_string(), work_dir.to_string_lossy().to_string()),
                ("ZED_CL_SYSTEM_INDEX".to_string(), system_index_name.to_string()),
                ("RUST_LOG".to_string(), "debug".to_string()),
            ];

            // Add custom environment variables from settings
            if let Some(custom_env) = settings
                .as_ref()
                .and_then(|s| s.get("env"))
                .and_then(|v| v.as_object())
            {
                for (key, value) in custom_env {
                    if let Some(value_str) = value.as_str() {
                        env.push((key.clone(), value_str.to_string()));
                    }
                }
            }

            Ok(zed::Command {
                command: lsp_binary.to_string_lossy().to_string(),
                args: vec![],
                env,
            })
        } else {
            Err(format!("Unknown language server: {}", language_server_id))
        }
    }
}

zed::register_extension!(CommonLispExtension);
