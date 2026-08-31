use zed_extension_api as zed;
use zed::settings::LspSettings;
use std::fs;
use std::path::{Path, PathBuf};

const GITHUB_REPO: &str = "etyurkin/zed-cl";
const LANGUAGE_SERVER_ID: &str = "common-lisp";
const WORKTREE_ENV_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
];

const ZED_CL_ASD: &str = include_str!("../../src/zed-cl-repl-impl/zed-cl.asd");
const START_MASTER_REPL: &str = include_str!("../../src/zed-cl-repl-impl/start-master-repl.lisp");
const BOOTSTRAP: &str = include_str!("../../src/zed-cl-repl-impl/bootstrap.lisp");
const COMPAT: &str = include_str!("../../src/zed-cl-repl-impl/compat.lisp");
const CONFIG: &str = include_str!("../../src/zed-cl-repl-impl/config.lisp");
const DISPLAY: &str = include_str!("../../src/zed-cl-repl-impl/display.lisp");
const SOCKET_SERVER: &str = include_str!("../../src/zed-cl-repl-impl/socket-server.lisp");
const MASTER_REPL: &str = include_str!("../../src/zed-cl-repl-impl/master-repl.lisp");

struct CommonLispExtension {
    cached_lsp_path: Option<String>,
    github_unavailable: bool,
}

impl CommonLispExtension {
    fn binary_name(name: &str) -> String {
        let (os, _) = zed::current_platform();
        match os {
            zed::Os::Windows => format!("{name}.exe"),
            _ => name.to_string(),
        }
    }

    fn local_binary(name: &str) -> Option<String> {
        let file = Self::binary_name(name);
        for path in [
            PathBuf::from("bin").join(&file),
            PathBuf::from(&file),
        ] {
            if path.is_file() {
                return Some(path.to_string_lossy().to_string());
            }
        }
        None
    }

    /// A binary left by a previous auto-download in zed-cl-<version>/, so a
    /// restart works without reaching GitHub.
    fn versioned_binary(name: &str) -> Option<String> {
        let file = Self::binary_name(name);
        let mut candidates: Vec<PathBuf> = fs::read_dir(".")
            .ok()?
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("zed-cl-"))
            })
            .map(|entry| entry.path().join(&file))
            .filter(|path| path.is_file())
            .collect();
        candidates.sort();
        candidates
            .pop()
            .map(|path| path.to_string_lossy().into_owned())
    }

    /// Path under ~/.zed-cl/bin, returned unchecked: the WASI sandbox cannot
    /// stat host paths, but Zed spawns the command on the host where it
    /// resolves. Do not add an is_file() guard here - it always fails in the
    /// sandbox and silently disables this fallback.
    fn home_binary(worktree: &zed::Worktree, name: &str) -> Option<String> {
        let home = Self::env_value(worktree, "HOME")
            .or_else(|| Self::env_value(worktree, "USERPROFILE"))?;
        Some(
            PathBuf::from(home)
                .join(".zed-cl")
                .join("bin")
                .join(Self::binary_name(name))
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn asset_name() -> String {
        let (os, arch) = zed::current_platform();
        let os = match os {
            zed::Os::Mac => "macos",
            zed::Os::Linux => "linux",
            zed::Os::Windows => "windows",
        };
        let arch = match arch {
            zed::Architecture::Aarch64 => "aarch64",
            zed::Architecture::X8664 => "x86_64",
            zed::Architecture::X86 => "x86",
        };
        format!("zed-cl-{os}-{arch}.zip")
    }

    fn download_binaries(&mut self, language_server_id: &zed::LanguageServerId) -> Result<String, String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            GITHUB_REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )
        .map_err(|e| {
            format!(
                "no GitHub release for {GITHUB_REPO} ({e}). Install zed-cl-lsp so it is on PATH."
            )
        })?;

        let asset_name = Self::asset_name();
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| {
                format!(
                    "No GitHub release asset named {asset_name}. Install zed-cl-lsp so it is on PATH, or build it with `make build`."
                )
            })?;

        let version_dir = format!("zed-cl-{}", release.version);
        fs::create_dir_all(&version_dir)
            .map_err(|e| format!("failed to create {version_dir}: {e}"))?;

        let lsp_path = format!("{version_dir}/{}", Self::binary_name("zed-cl-lsp"));
        if !Path::new(&lsp_path).is_file() {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::download_file(
                &asset.download_url,
                &version_dir,
                zed::DownloadedFileType::Zip,
            )
            .map_err(|e| format!("failed to download {asset_name}: {e}"))?;

            for name in ["zed-cl-lsp", "zed-cl-kernel", "zed-cl-index", "zed-cl-repl"] {
                let path = format!("{version_dir}/{}", Self::binary_name(name));
                let _ = zed::make_file_executable(&path);
            }
        }

        self.cached_lsp_path = Some(lsp_path.clone());
        Ok(lsp_path)
    }

    fn language_server_binary_path(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String, String> {
        if let Some(path) = worktree.which(&Self::binary_name("zed-cl-lsp")) {
            return Ok(path);
        }
        if let Some(path) = Self::local_binary("zed-cl-lsp") {
            return Ok(path);
        }
        if let Some(path) = &self.cached_lsp_path {
            if Path::new(path).is_file() {
                return Ok(path.clone());
            }
        }
        if let Some(path) = Self::versioned_binary("zed-cl-lsp") {
            self.cached_lsp_path = Some(path.clone());
            return Ok(path);
        }
        if !self.github_unavailable {
            match self.download_binaries(language_server_id) {
                Ok(path) => return Ok(path),
                Err(_) => self.github_unavailable = true,
            }
        }
        if let Some(path) = Self::home_binary(worktree, "zed-cl-lsp") {
            return Ok(path);
        }
        Err(
            "zed-cl-lsp not found on PATH or in the extension work directory, and it could not be downloaded from GitHub."
                .to_string(),
        )
    }

    fn env_value(worktree: &zed::Worktree, key: &str) -> Option<String> {
        worktree
            .shell_env()
            .into_iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    fn write_repl_sources(&self, work_dir: &Path) -> Result<(), String> {
        let repl_dir = work_dir.join("repl");
        fs::create_dir_all(&repl_dir)
            .map_err(|e| format!("Failed to create repl directory: {e}"))?;
        let write = |name: &str, content: &str| -> Result<(), String> {
            fs::write(repl_dir.join(name), content)
                .map_err(|e| format!("Failed to write {name}: {e}"))
        };
        write("zed-cl.asd", ZED_CL_ASD)?;
        write("start-master-repl.lisp", START_MASTER_REPL)?;
        write("bootstrap.lisp", BOOTSTRAP)?;
        write("compat.lisp", COMPAT)?;
        write("config.lisp", CONFIG)?;
        write("display.lisp", DISPLAY)?;
        write("socket-server.lisp", SOCKET_SERVER)?;
        write("master-repl.lisp", MASTER_REPL)?;
        Ok(())
    }
}

impl zed::Extension for CommonLispExtension {
    fn new() -> Self {
        Self {
            cached_lsp_path: None,
            github_unavailable: false,
        }
    }

    fn label_for_completion(
        &self,
        _language_server_id: &zed::LanguageServerId,
        completion: zed::lsp::Completion,
    ) -> Option<zed::CodeLabel> {
        use zed::lsp::CompletionKind;
        use zed::CodeLabelSpan;

        let label = &completion.label;
        let detail = completion.detail.as_ref().map(|s| s.as_str()).unwrap_or("");

        let (package, name) = if label.starts_with('[') {
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

        let mut code = String::new();
        let mut spans = vec![];

        if !package.is_empty() {
            code.push('[');
            spans.push(CodeLabelSpan::literal("[", None));
            code.push_str(package);
            spans.push(CodeLabelSpan::literal(package, Some("constant".to_string())));
            code.push(']');
            spans.push(CodeLabelSpan::literal("]", None));
            if !name.is_empty() {
                code.push(' ');
                spans.push(CodeLabelSpan::literal(" ", None));
            }
        }

        if !name.is_empty() {
            code.push_str(name);
            let name_highlight = match completion.kind {
                Some(CompletionKind::Function) | Some(CompletionKind::Method) => Some("function".to_string()),
                Some(CompletionKind::Variable) | Some(CompletionKind::Constant) => Some("variable".to_string()),
                Some(CompletionKind::Module) => Some("constant".to_string()),
                _ => None,
            };
            spans.push(CodeLabelSpan::literal(name, name_highlight));
        }

        if !detail.is_empty() {
            code.push(' ');
            spans.push(CodeLabelSpan::literal(" ", None));
        }

        if let Some(paren_pos) = detail.find('(') {
            let kind = detail[..paren_pos].trim_end();
            let params_with_parens = &detail[paren_pos..];
            code.push_str(kind);
            spans.push(CodeLabelSpan::literal(kind.to_string(), Some("keyword".to_string())));
            code.push(' ');
            spans.push(CodeLabelSpan::literal(" ", None));
            code.push('(');
            spans.push(CodeLabelSpan::literal("(", Some("constant".to_string())));
            if let Some(close_paren_pos) = params_with_parens.find(')') {
                let params_inside = &params_with_parens[1..close_paren_pos];
                code.push_str(params_inside);
                spans.push(CodeLabelSpan::literal(params_inside.to_string(), None));
                code.push(')');
                spans.push(CodeLabelSpan::literal(")", Some("constant".to_string())));
            }
        } else {
            code.push_str(detail);
            spans.push(CodeLabelSpan::literal(detail.to_string(), Some("keyword".to_string())));
        }

        let filter_end = if !package.is_empty() {
            1 + package.len() + 2 + name.len()
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
        if language_server_id.as_ref() != LANGUAGE_SERVER_ID {
            return Err(format!("Unknown language server: {}", language_server_id));
        }

        let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.write_repl_sources(&work_dir)?;

        let lsp_binary = self.language_server_binary_path(language_server_id, worktree)?;

        let settings = LspSettings::for_worktree(LANGUAGE_SERVER_ID, worktree)
            .ok()
            .and_then(|s| s.settings);

        let mut env = vec![
            (
                "ZED_CL_EXTENSION_DIR".to_string(),
                work_dir.to_string_lossy().to_string(),
            ),
            ("RUST_LOG".to_string(), "info".to_string()),
        ];

        for key in WORKTREE_ENV_KEYS {
            if let Some(value) = Self::env_value(worktree, key) {
                env.push((key.to_string(), value));
            }
        }

        if let Some(system_index_name) = settings
            .as_ref()
            .and_then(|s| s.get("system_index"))
            .and_then(|v| v.as_str())
        {
            env.push(("ZED_CL_SYSTEM_INDEX".to_string(), system_index_name.to_string()));
        }

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
            command: lsp_binary,
            args: vec![],
            env,
        })
    }
}

zed::register_extension!(CommonLispExtension);
