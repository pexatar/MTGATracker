//! Local AI engine driven as a `llama-server` sidecar (from llama.cpp).
//!
//! Rather than compiling llama.cpp into the app, we run the official
//! pre-built `llama-server` binary as a child process and talk to it over its
//! local **OpenAI-compatible** HTTP API. This keeps the app portable (binary +
//! GGUF model live next to it, e.g. on a USB stick), needs no C++ build
//! toolchain, and lets the same connector serve both the local engine and any
//! cloud OpenAI-compatible provider later.
//!
//! GPU is used automatically when the binary is a CUDA build (`-ngl` offloads
//! layers); on a CPU-only build `-ngl` is a no-op, so the same call falls back
//! to CPU transparently.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
/// Windows: start the sidecar without opening a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Loopback host the sidecar binds to (never exposed outside the machine).
const HOST: &str = "127.0.0.1";
/// Default local port for the sidecar. Uncommon value to avoid clashes;
/// configurable later from Settings.
const PORT: u16 = 49669;
/// Context window passed to the model.
const CTX_SIZE: u32 = 4096;
/// Max seconds to wait for the model to load and the server to answer /health.
const STARTUP_TIMEOUT_SECS: u64 = 180;

/// Handle to the running sidecar process, kept alive across commands.
fn child_slot() -> &'static Mutex<Option<Child>> {
    static SLOT: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Status of the local AI engine, surfaced to the UI.
#[derive(Serialize)]
pub struct AiStatus {
    /// Whether the `llama-server` binary was found.
    pub binary_found: bool,
    /// Whether a `.gguf` model file was found.
    pub model_found: bool,
    /// File name of the model that would be used, if any.
    pub model_name: Option<String>,
    /// Whether the sidecar is currently answering on the local port.
    pub running: bool,
}

/// Resolved paths to the sidecar binary and the model file.
struct AiPaths {
    binary: Option<PathBuf>,
    model: Option<PathBuf>,
}

/// Platform-specific name of the sidecar binary.
fn binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Directories searched for the engine, in order: an `ai/` folder next to the
/// executable (portable layout, e.g. on a USB stick), then an `ai/` folder in
/// the app data directory (handy during development). Nothing is hard-coded to
/// an absolute path.
fn candidate_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("ai"));
        }
    }
    if let Ok(data) = app.path().app_data_dir() {
        dirs.push(data.join("ai"));
    }
    dirs
}

/// Finds the first `.gguf` file in a directory, if any.
fn first_gguf(dir: &std::path::Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("gguf"))
            == Some(true)
        {
            return Some(path);
        }
    }
    None
}

/// Locates the sidecar binary and the model file across the candidate dirs.
fn locate(app: &AppHandle) -> AiPaths {
    let mut binary = None;
    let mut model = None;
    for dir in candidate_dirs(app) {
        if binary.is_none() {
            let candidate = dir.join(binary_name());
            if candidate.is_file() {
                binary = Some(candidate);
            }
        }
        if model.is_none() {
            if let Some(gguf) = first_gguf(&dir) {
                model = Some(gguf);
            }
        }
    }
    AiPaths { binary, model }
}

/// Base URL of the local sidecar API.
fn base_url() -> String {
    format!("http://{HOST}:{PORT}")
}

/// Returns true if the sidecar already answers on the local port.
async fn is_healthy() -> bool {
    let client = reqwest::Client::new();
    matches!(
        client
            .get(format!("{}/health", base_url()))
            .send()
            .await,
        Ok(resp) if resp.status().is_success()
    )
}

/// Current engine status (files present + server reachable).
pub async fn status(app: &AppHandle) -> AiStatus {
    let paths = locate(app);
    AiStatus {
        binary_found: paths.binary.is_some(),
        model_found: paths.model.is_some(),
        model_name: paths
            .model
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string()),
        running: is_healthy().await,
    }
}

/// Ensures the sidecar is running, starting it if needed, and waits until it
/// answers /health. Returns the base URL of the API.
async fn ensure_running(app: &AppHandle) -> Result<String, String> {
    if is_healthy().await {
        return Ok(base_url());
    }

    let paths = locate(app);
    let binary = paths
        .binary
        .ok_or_else(|| format!("AI engine not found: place '{}' in an 'ai' folder next to the app.", binary_name()))?;
    let model = paths
        .model
        .ok_or("AI model not found: place a .gguf model file in the 'ai' folder.")?;

    // Spawn the sidecar. The lock is held only for the quick spawn, never
    // across an await.
    {
        let mut slot = child_slot().lock().map_err(|e| e.to_string())?;
        // Drop a previous handle that is no longer healthy.
        if let Some(mut old) = slot.take() {
            let _ = old.kill();
        }
        let mut cmd = std::process::Command::new(&binary);
        cmd.arg("-m")
            .arg(&model)
            .arg("--host")
            .arg(HOST)
            .arg("--port")
            .arg(PORT.to_string())
            .arg("--ctx-size")
            .arg(CTX_SIZE.to_string())
            // Offload everything to GPU when the binary supports it; on a
            // CPU-only build this is ignored (automatic CPU fallback).
            .arg("-ngl")
            .arg("999");
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let child = cmd
            .spawn()
            .map_err(|e| format!("Could not start the AI engine: {e}"))?;
        *slot = Some(child);
    }

    // Wait for the model to load and the server to become healthy.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(STARTUP_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        if is_healthy().await {
            return Ok(base_url());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err("The AI engine did not become ready in time.".to_string())
}

/// Sends a single prompt to the local model and returns its text reply.
/// Used for the first manual test of the engine; the real analysis features
/// will build on this.
pub async fn chat(app: &AppHandle, prompt: &str) -> Result<String, String> {
    let url = ensure_running(app).await?;
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "model": "local",
        "messages": [{ "role": "user", "content": prompt }],
        "stream": false
    });
    let resp = client
        .post(format!("{url}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("AI request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("AI engine returned status {}", resp.status()));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid AI response: {e}"))?;
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "The AI response had no content.".to_string())
}

/// Stops the sidecar if it is running (best effort).
pub fn stop() {
    if let Ok(mut slot) = child_slot().lock() {
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
        }
    }
}
