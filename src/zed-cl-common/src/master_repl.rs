/// Client for communicating with the master REPL over TCP localhost.
/// Frames are 4-byte big-endian UTF-8 length followed by that many bytes of a printed sexp.

use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{self, BufReader, Read, Write};
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
use crate::protocol::{ReplRequest, ReplResponse, ResponseData};

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

const CONNECT_ATTEMPTS: u32 = 50;
const SPAWN_CONNECT_ATTEMPTS: u32 = 225;
const CONNECT_RETRY: Duration = Duration::from_millis(200);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FRAME: usize = 32 * 1024 * 1024;

pub fn connection_file_path() -> PathBuf {
    Profile::get().connection_file_path()
}

fn spawn_lock_path() -> PathBuf {
    data_dir().join(format!("repl-{}.lock", Profile::get().lisp_impl))
}

struct SpawnLock {
    path: PathBuf,
}

impl Drop for SpawnLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_pid_alive(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return false;
    };
    pid_is_alive(pid)
}

fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        windows_pid_alive(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

#[cfg(windows)]
fn windows_pid_alive(pid: u32) -> bool {
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ok && code == STILL_ACTIVE
    }
}

fn try_acquire_spawn_lock() -> Option<SpawnLock> {
    let path = spawn_lock_path();
    let _ = std::fs::create_dir_all(data_dir());
    for _ in 0..3 {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = write!(file, "{}", std::process::id());
                return Some(SpawnLock { path });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if lock_pid_alive(&path) {
                    return None;
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => {
                error!("Failed to acquire REPL spawn lock: {}", e);
                return None;
            }
        }
    }
    None
}

fn enable_keepalive(stream: &TcpStream) {
    #[cfg(unix)]
    unsafe {
        let yes: libc::c_int = 1;
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_KEEPALIVE,
            &yes as *const _ as *const libc::c_void,
            std::mem::size_of_val(&yes) as libc::socklen_t,
        );
    }
    let _ = stream;
}

fn peer_closed(stream: &TcpStream) -> bool {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 1];
        let n = unsafe {
            libc::recv(
                stream.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        if n == 0 {
            return true;
        }
        if n < 0 {
            let err = io::Error::last_os_error();
            return !matches!(
                err.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
            );
        }
        false
    }
    #[cfg(not(unix))]
    {
        stream.peer_addr().is_err()
    }
}

struct ReplStream {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl ReplStream {
    fn from_tcp(stream: TcpStream) -> Result<Self> {
        stream.set_nodelay(true)?;
        enable_keepalive(&stream);
        let writer = stream.try_clone().context("clone TCP stream")?;
        Ok(Self {
            reader: BufReader::with_capacity(64 * 1024, stream),
            writer,
        })
    }

    fn write_frame(&mut self, sexp: &str) -> Result<()> {
        write_frame(&mut self.writer, sexp)
    }

    fn read_frame(&mut self) -> Result<String> {
        read_frame(&mut self.reader)
    }

    fn is_broken(&self) -> bool {
        self.writer.take_error().ok().flatten().is_some()
            || self.writer.peer_addr().is_err()
            || peer_closed(&self.writer)
    }
}

pub fn write_frame<W: Write>(stream: &mut W, sexp: &str) -> Result<()> {
    let bytes = sexp.as_bytes();
    let len = u32::try_from(bytes.len()).context("frame too large")?;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()?;
    Ok(())
}

pub fn read_frame<R: Read>(stream: &mut R) -> Result<String> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .context("Failed to read frame length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        anyhow::bail!("frame too large: {len}");
    }
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .context("Failed to read frame body")?;
    String::from_utf8(buf).context("frame is not UTF-8")
}

pub struct MasterReplClient {
    stream: Option<ReplStream>,
    request_counter: u64,
    extension_dir: Option<PathBuf>,
    repl_starting: Arc<AtomicBool>,
    spawn_lock: Option<SpawnLock>,
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
            spawn_lock: None,
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
        Self::try_ready_stream().is_some()
    }

    fn try_connect() -> Option<TcpStream> {
        let conn = Profile::get().read_connection()?;
        let stream = TcpStream::connect((conn.host.as_str(), conn.port)).ok()?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
        stream.set_nodelay(true).ok()?;
        Some(stream)
    }

    pub fn try_start_master_repl(&mut self) {
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

        let Some(lock) = try_acquire_spawn_lock() else {
            debug!("Another process is starting the master REPL");
            self.repl_starting.store(false, Ordering::SeqCst);
            return;
        };

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
                self.spawn_lock = Some(lock);
            }
            Err(e) => {
                error!("Failed to spawn master REPL: {}", e);
                self.repl_starting.store(false, Ordering::SeqCst);
            }
        }
    }

    fn wait_for_connection(&mut self) -> Result<ReplStream> {
        let mut attempts = 0u32;
        let mut max = CONNECT_ATTEMPTS;
        loop {
            if let Some(stream) = Self::try_ready_stream() {
                self.spawn_lock = None;
                self.repl_starting.store(false, Ordering::SeqCst);
                return Ok(stream);
            }
            if attempts == 0 {
                self.try_start_master_repl();
                if self.repl_starting.load(Ordering::SeqCst) || lock_pid_alive(&spawn_lock_path()) {
                    max = SPAWN_CONNECT_ATTEMPTS;
                }
            }
            attempts += 1;
            if attempts >= max {
                break;
            }
            std::thread::sleep(CONNECT_RETRY);
        }
        self.spawn_lock = None;
        self.repl_starting.store(false, Ordering::SeqCst);
        if Self::try_connect().is_some() {
            anyhow::bail!(
                "Master REPL is listening but did not complete a framed ping. Restart the old SBCL process. Log: {}",
                log_dir().join("master-repl.log").display()
            );
        }
        anyhow::bail!(
            "Failed to connect to master REPL after {}s. Check {} and install SBCL.",
            attempts as u64 * CONNECT_RETRY.as_millis() as u64 / 1000,
            log_dir().join("master-repl.log").display()
        )
    }

    fn try_ready_stream() -> Option<ReplStream> {
        let tcp = Self::try_connect()?;
        let mut stream = ReplStream::from_tcp(tcp).ok()?;
        stream.write_frame("(:type \"ping\" :id \"handshake\")").ok()?;
        let response = stream.read_frame().ok()?;
        let parsed = ReplResponse::from_sexp(&response, "handshake").ok()?;
        matches!(parsed.data, ResponseData::Pong).then_some(stream)
    }

    fn ensure_connected(&mut self) -> Result<()> {
        if let Some(stream) = self.stream.as_ref() {
            if !stream.is_broken() {
                return Ok(());
            }
            self.stream = None;
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
        self.stream
            .as_ref()
            .is_some_and(|s| !s.is_broken())
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

        match self.write_and_read(&sexp, &request_id) {
            Ok(response) => Ok(response),
            Err(e) => {
                debug!("Request failed ({e:#}), reconnecting");
                self.stream = None;
                self.ensure_connected()?;
                self.write_and_read(&sexp, &request_id)
                    .with_context(|| format!("Failed after reconnect: {e:#}"))
            }
        }
    }

    fn write_and_read(&mut self, sexp: &str, request_id: &str) -> Result<ReplResponse> {
        let stream = self.stream.as_mut().context("Not connected")?;
        stream.write_frame(sexp)?;
        let response = stream.read_frame()?;
        debug!("Received response: {}", response.trim());
        ReplResponse::from_sexp(&response, request_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrips_frame() {
        let mut buf = Vec::new();
        write_frame(&mut buf, r#"(:ID "1" :PONG T)"#).unwrap();
        let mut cur = Cursor::new(buf);
        let text = read_frame(&mut cur).unwrap();
        assert_eq!(text, r#"(:ID "1" :PONG T)"#);
    }
}
