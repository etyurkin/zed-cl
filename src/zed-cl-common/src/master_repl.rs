/// Client for communicating with the master REPL over TCP localhost.
/// Works on macOS, Linux, and Windows.

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tracing::{debug, error, info};

use crate::config::{data_dir, log_dir, Profile};
use crate::protocol::{ReplRequest, ReplResponse};

const CONNECT_ATTEMPTS: u32 = 50;
const CONNECT_RETRY: Duration = Duration::from_millis(200);
const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub fn connection_file_path() -> PathBuf {
    Profile::get().connection_file_path()
}

pub struct MasterReplClient {
    stream: Option<TcpStream>,
    request_counter: u64,
    extension_dir: Option<PathBuf>,
    repl_starting: Arc<AtomicBool>,
}

impl MasterReplClient {
    pub fn new() -> Self {
        let extension_dir = std::env::var("ZED_CL_EXTENSION_DIR")
            .ok()
            .map(PathBuf::from);

        Self {
            stream: None,
            request_counter: 0,
            extension_dir,
            repl_starting: Arc::new(AtomicBool::new(false)),
        }
    }

    fn expand_tilde(path: &PathBuf) -> PathBuf {
        if let Some(path_str) = path.to_str() {
            if let Some(rest) = path_str.strip_prefix("~/") {
                if let Some(home) = crate::config::home_dir() {
                    return home.join(rest);
                }
            } else if path_str == "~" {
                if let Some(home) = crate::config::home_dir() {
                    return home;
                }
            }
        }
        path.clone()
    }

    fn is_master_repl_alive(&self) -> bool {
        Self::try_connect().is_some()
    }

    fn try_connect() -> Option<TcpStream> {
        let conn = Profile::get().read_connection()?;
        let stream = TcpStream::connect((conn.host.as_str(), conn.port)).ok()?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
        stream.set_nodelay(true).ok()?;
        Some(stream)
    }

    pub fn try_start_master_repl(&self) {
        if self
            .repl_starting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            debug!("Master REPL already starting, skipping duplicate spawn");
            return;
        }

        if self.is_master_repl_alive() {
            debug!("Master REPL already running");
            self.repl_starting.store(false, Ordering::SeqCst);
            return;
        }

        info!("Master REPL not running, starting in background...");

        let ext_dir = match self.extension_dir.as_ref() {
            Some(dir) => dir,
            None => {
                error!("ZED_CL_EXTENSION_DIR not set, cannot auto-start master REPL");
                self.repl_starting.store(false, Ordering::SeqCst);
                return;
            }
        };

        let ext_dir = Self::expand_tilde(ext_dir);
        let config = Profile::get();
        let lisp_impl = &config.lisp_impl;

        let (lisp_cmd, lisp_args) = match lisp_impl.as_str() {
            "sbcl" => match find_executable("sbcl") {
                Some(path) => (path, vec!["--noinform", "--no-userinit", "--load"]),
                None => {
                    error!("SBCL not found in PATH. Install SBCL and restart Zed.");
                    self.repl_starting.store(false, Ordering::SeqCst);
                    return;
                }
            },
            "ecl" => match find_executable("ecl") {
                Some(path) => (path, vec!["-norc", "-load"]),
                None => {
                    error!("ECL not found in PATH (lisp_impl=ecl).");
                    self.repl_starting.store(false, Ordering::SeqCst);
                    return;
                }
            },
            other => {
                error!("Unsupported Lisp implementation: {}. Supported: sbcl, ecl", other);
                self.repl_starting.store(false, Ordering::SeqCst);
                return;
            }
        };

        let start_script = ext_dir.join("repl").join("start-master-repl.lisp");
        if !start_script.exists() {
            error!("Master REPL startup script not found at {:?}", start_script);
            self.repl_starting.store(false, Ordering::SeqCst);
            return;
        }

        let _ = std::fs::create_dir_all(data_dir());
        let log_path = log_dir().join("master-repl.log");
        let log_file = match std::fs::File::create(&log_path) {
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

        info!("Spawning master REPL with {:?}...", lisp_cmd);

        let mut cmd = Command::new(&lisp_cmd);
        for arg in lisp_args {
            cmd.arg(arg);
        }
        cmd.arg(&start_script)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_file_err));

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
            const DETACHED_PROCESS: u32 = 0x00000008;
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        match cmd.spawn() {
            Ok(child) => {
                info!(
                    "Master REPL spawned with PID: {} (log: {})",
                    child.id(),
                    log_path.display()
                );
            }
            Err(e) => {
                error!("Failed to spawn master REPL: {}", e);
            }
        }
        self.repl_starting.store(false, Ordering::SeqCst);
    }

    fn wait_for_connection(&mut self) -> Result<TcpStream> {
        for attempt in 1..=CONNECT_ATTEMPTS {
            if let Some(stream) = Self::try_ready_stream() {
                return Ok(stream);
            }
            if attempt == 1 {
                self.try_start_master_repl();
            }
            std::thread::sleep(CONNECT_RETRY);
        }
        anyhow::bail!(
            "Failed to connect to master REPL after {}s. Check {} and install SBCL.",
            CONNECT_ATTEMPTS as u64 * CONNECT_RETRY.as_millis() as u64 / 1000,
            log_dir().join("master-repl.log").display()
        )
    }

    fn try_ready_stream() -> Option<TcpStream> {
        let mut stream = Self::try_connect()?;
        write_sexp(&mut stream, "(:type \"ping\" :id \"handshake\")").ok()?;
        let response = read_sexp(&mut stream).ok()?;
        if response.to_uppercase().contains("PONG") {
            Some(stream)
        } else {
            None
        }
    }

    fn ensure_connected(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        let stream = self.wait_for_connection()?;
        debug!("Connected to master REPL");
        self.stream = Some(stream);
        Ok(())
    }

    pub fn connect(&mut self) -> Result<()> {
        debug!(
            "Connecting to master REPL via {:?}",
            connection_file_path()
        );
        self.ensure_connected()
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    fn next_request_id(&mut self) -> String {
        self.request_counter += 1;
        format!("rust-{}", self.request_counter)
    }

    pub fn send_request(&mut self, mut request: ReplRequest) -> Result<ReplResponse> {
        self.ensure_connected()?;
        self.assign_id(&mut request);
        let request_id = request.id().to_string();
        let sexp = request.to_sexp();
        debug!("Sending request: {}", sexp);

        if let Err(_e) = write_sexp(self.stream.as_mut().context("Not connected")?, &sexp) {
            self.stream = None;
            self.ensure_connected()?;
            write_sexp(self.stream.as_mut().context("Not connected")?, &sexp)
                .context("Failed to send request after reconnect")?;
        }

        match read_sexp(self.stream.as_mut().context("Not connected")?) {
            Ok(response) => {
                debug!("Received response: {}", response.trim());
                ReplResponse::from_sexp(&response, &request_id)
            }
            Err(e) => {
                self.stream = None;
                Err(e)
            }
        }
    }

    fn assign_id(&mut self, request: &mut ReplRequest) {
        let id = request.id_mut();
        if id.is_empty() {
            *id = self.next_request_id();
        }
    }

    pub fn close(&mut self) -> Result<()> {
        debug!("Closing TCP connection");
        self.stream.take();
        Ok(())
    }
}

fn write_sexp(stream: &mut TcpStream, sexp: &str) -> Result<()> {
    stream.write_all(sexp.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn read_sexp(stream: &mut TcpStream) -> Result<String> {
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
                    '\\' if in_string => escape_next = true,
                    '"' => in_string = !in_string,
                    '(' if !in_string => {
                        paren_depth += 1;
                        started = true;
                    }
                    ')' if !in_string => {
                        paren_depth -= 1;
                        if started && paren_depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => {
                return Err(e).context("Failed to read response");
            }
        }
    }

    if response.is_empty() {
        anyhow::bail!("Connection closed by master REPL");
    }
    Ok(response)
}

fn find_executable(name: &str) -> Option<PathBuf> {
    which::which(name)
        .or_else(|_| which::which(format!("{name}.exe")))
        .ok()
        .or_else(|| {
            extra_install_dirs()
                .into_iter()
                .find_map(|dir| locate_in_dir(&dir, name))
        })
}

fn locate_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let candidates = [name.to_string(), format!("{name}.exe")];
    for file in &candidates {
        let direct = dir.join(file);
        if direct.is_file() {
            return Some(direct);
        }
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        for file in &candidates {
            let nested = entry.path().join(file);
            if nested.is_file() {
                return Some(nested);
            }
        }
    }
    None
}

fn extra_install_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = crate::config::home_dir() {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("scoop/shims"));
        dirs.push(home.join("scoop/apps/sbcl/current"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/usr/bin"));
    dirs.push(PathBuf::from(r"C:\Program Files\Steel Bank Common Lisp"));
    dirs.push(PathBuf::from(r"C:\Program Files (x86)\Steel Bank Common Lisp"));
    dirs
}

impl Default for MasterReplClient {
    fn default() -> Self {
        Self::new()
    }
}
