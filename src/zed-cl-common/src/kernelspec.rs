/// Jupyter kernelspec registration for Zed's REPL.
///
/// The Zed extension runs in a WASI sandbox and may only touch its own work
/// directory, so it cannot write the kernelspec itself. The LSP is spawned by
/// Zed as a normal process and inherits the environment the extension passes
/// it, which makes it the right place to do this.

use serde_json::json;
use std::path::{Path, PathBuf};

const KERNEL_NAME: &str = "commonlisp-zed";

/// Environment the kernel needs to find SBCL and the user's home directory.
/// Jupyter does not pass the parent environment through, so it is baked in.
const KERNEL_ENV_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
];

fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Where Jupyter looks for kernelspecs on this platform.
pub fn kernels_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        crate::home_dir()?.join("Library").join("Jupyter")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("jupyter")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::home_dir().unwrap_or_default().join(".local/share"))
            .join("jupyter")
    };
    Some(base.join("kernels").join(KERNEL_NAME))
}

/// The kernel ships alongside the LSP, so locate it relative to this binary and
/// fall back to the install directory.
fn kernel_binary() -> Option<PathBuf> {
    let file = binary_name("zed-cl-kernel");
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file)));
    if let Some(path) = sibling.filter(|p| p.is_file()) {
        return Some(path);
    }
    let installed = crate::data_dir().join("bin").join(&file);
    installed.is_file().then_some(installed)
}

fn kernel_json(kernel: &Path, extension_dir: Option<&str>) -> serde_json::Value {
    let mut env = serde_json::Map::new();
    env.insert("RUST_LOG".into(), json!("info"));
    if let Some(dir) = extension_dir {
        env.insert("ZED_CL_EXTENSION_DIR".into(), json!(dir.replace('\', "/")));
    }
    for key in KERNEL_ENV_KEYS {
        if let Some(value) = std::env::var_os(key).and_then(|v| v.into_string().ok()) {
            env.insert((*key).into(), json!(value));
        }
    }
    json!({
        "display_name": "Common Lisp (Zed)",
        "language": "Common Lisp",
        "argv": [kernel.to_string_lossy().replace('\', "/"), "{connection_file}"],
        "env": env,
        "interrupt_mode": "message",
        "metadata": { "debugger": false }
    })
}

/// Write `kernel.json` so Zed's REPL can discover the kernel. Rewrites only on
/// change, so a restart with an unchanged environment leaves the file alone.
pub fn register() -> anyhow::Result<PathBuf> {
    let dir = kernels_dir().ok_or_else(|| anyhow::anyhow!("no Jupyter data directory"))?;
    let kernel = kernel_binary()
        .ok_or_else(|| anyhow::anyhow!("zed-cl-kernel not found next to zed-cl-lsp"))?;
    let extension_dir = std::env::var("ZED_CL_EXTENSION_DIR").ok();
    let body = serde_json::to_string_pretty(&kernel_json(&kernel, extension_dir.as_deref()))?;

    let path = dir.join("kernel.json");
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == body) {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&path, body)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_json_uses_forward_slashes_and_connection_file() {
        let value = kernel_json(Path::new(r"C:\tools\zed-cl-kernel.exe"), Some(r"C:\work\cl"));
        let argv = value["argv"].as_array().unwrap();
        assert_eq!(argv[0], json!("C:/tools/zed-cl-kernel.exe"));
        assert_eq!(argv[1], json!("{connection_file}"));
        assert_eq!(value["env"]["ZED_CL_EXTENSION_DIR"], json!("C:/work/cl"));
        assert_eq!(value["interrupt_mode"], json!("message"));
    }

    #[test]
    fn kernel_json_omits_extension_dir_when_absent() {
        let value = kernel_json(Path::new("/usr/local/bin/zed-cl-kernel"), None);
        assert!(value["env"].get("ZED_CL_EXTENSION_DIR").is_none());
        assert_eq!(value["env"]["RUST_LOG"], json!("info"));
    }
}
