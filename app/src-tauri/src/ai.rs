//! Local AI engine driven as a `llama-server` sidecar (from llama.cpp).
//!
//! Rather than compiling llama.cpp into the app, we run the official pre-built
//! `llama-server` binary as a child process and talk to it over its local
//! **OpenAI-compatible** HTTP API. This keeps the app portable (binary + GGUF
//! model live in an `ai` folder next to it, e.g. on a USB stick), needs no C++
//! build toolchain, and lets the same connector serve both the local engine and
//! any cloud OpenAI-compatible provider later.
//!
//! GPU is used automatically when the binary is a CUDA build (`-ngl` offloads
//! layers); on a CPU-only build `-ngl` is a no-op, so the same call falls back
//! to CPU transparently.
//!
//! The sidecar binds to a **free port chosen at runtime** (asked from the OS):
//! a fixed port is fragile because Windows can already be using it for its own
//! dynamic services.

use futures_util::StreamExt;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
/// Windows: start the sidecar without opening a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Loopback host the sidecar binds to (never exposed outside the machine).
const HOST: &str = "127.0.0.1";
/// Context window passed to the model. Generous: deck-analysis prompts list
/// every card (~3k tokens for a 100-card deck) and leave room for the answer
/// (and for the model's thinking, on the calls where it stays enabled).
const CTX_SIZE: u32 = 16384;
/// Max seconds to wait for the model to load and the server to answer /health.
const STARTUP_TIMEOUT_SECS: u64 = 180;

/// The running sidecar: its process handle and the port it bound to.
struct Running {
    child: Child,
    port: u16,
}

/// Handle to the running sidecar, kept alive across commands.
fn slot() -> &'static Mutex<Option<Running>> {
    static SLOT: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// The port of the currently tracked sidecar, if any.
fn current_port() -> Option<u16> {
    slot().lock().ok().and_then(|s| s.as_ref().map(|r| r.port))
}

/// Asks the OS for a free TCP port on the loopback interface.
fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind((HOST, 0))
        .map_err(|e| format!("Could not find a free port: {e}"))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| format!("Could not read the local port: {e}"))
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
    /// Whether the sidecar is currently answering on its local port.
    pub running: bool,
}

/// A streamed piece of the AI reply, emitted to the UI as an `ai-delta` event.
#[derive(Clone, Serialize)]
struct AiDelta {
    /// "reasoning" for the model's internal thinking, "content" for the answer.
    kind: String,
    /// The incremental text.
    text: String,
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

/// Base URL of the sidecar API for a given port.
fn url(port: u16) -> String {
    format!("http://{HOST}:{port}")
}

/// Returns true if a sidecar answers /health on the given port.
async fn is_healthy(port: u16) -> bool {
    let client = reqwest::Client::new();
    matches!(
        client.get(format!("{}/health", url(port))).send().await,
        Ok(resp) if resp.status().is_success()
    )
}

/// Current engine status (files present + server reachable).
pub async fn status(app: &AppHandle) -> AiStatus {
    let paths = locate(app);
    let running = match current_port() {
        Some(p) => is_healthy(p).await,
        None => false,
    };
    AiStatus {
        binary_found: paths.binary.is_some(),
        model_found: paths.model.is_some(),
        model_name: paths
            .model
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string()),
        running,
    }
}

/// Ensures the sidecar is running, starting it on a free port if needed, and
/// waits until it answers /health. Returns the base URL of the API.
async fn ensure_running(app: &AppHandle) -> Result<String, String> {
    if let Some(p) = current_port() {
        if is_healthy(p).await {
            return Ok(url(p));
        }
    }

    let paths = locate(app);
    let binary = paths.binary.ok_or_else(|| {
        format!("AI engine not found: place '{}' in an 'ai' folder next to the app.", binary_name())
    })?;
    let model = paths
        .model
        .ok_or("AI model not found: place a .gguf model file in the 'ai' folder.")?;
    let port = free_port()?;

    // Spawn the sidecar. The lock is held only for the quick spawn, never
    // across an await.
    {
        let mut guard = slot().lock().map_err(|e| e.to_string())?;
        if let Some(mut old) = guard.take() {
            let _ = old.child.kill();
        }
        let mut cmd = std::process::Command::new(&binary);
        cmd.arg("-m")
            .arg(&model)
            .arg("--host")
            .arg(HOST)
            .arg("--port")
            .arg(port.to_string())
            .arg("--ctx-size")
            .arg(CTX_SIZE.to_string())
            // Offload everything to GPU when the binary supports it; on a
            // CPU-only build this is ignored (automatic CPU fallback).
            .arg("-ngl")
            .arg("999")
            // Flash attention + quantized KV cache so a large context (16k)
            // fits in VRAM and runs fast (ignored gracefully if unsupported).
            .arg("--flash-attn")
            .arg("on")
            .arg("--cache-type-k")
            .arg("q8_0")
            .arg("--cache-type-v")
            .arg("q8_0");
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let child = cmd
            .spawn()
            .map_err(|e| format!("Could not start the AI engine: {e}"))?;
        *guard = Some(Running { child, port });
    }

    // Wait for the model to load and the server to become healthy.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(STARTUP_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        if is_healthy(port).await {
            return Ok(url(port));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err("The AI engine did not become ready in time.".to_string())
}

/// Streams a prompt to the local model. As text arrives it emits `ai-delta`
/// events (`{kind, text}`, where `kind` is "reasoning" for the model's internal
/// thinking — e.g. Gemma's — or "content" for the actual answer), and an
/// `ai-done` event when finished. This keeps the UI responsive instead of
/// blocking on a single long reply.
pub async fn chat_stream(app: &AppHandle, prompt: &str, think: bool) -> Result<(), String> {
    let base = ensure_running(app).await?;
    let client = reqwest::Client::new();
    // `max_tokens` caps reasoning + answer together. With reasoning on, an
    // in-depth analysis needs a large budget so it is not truncated mid-answer;
    // without it, replies are short. Both stay well within the 16k context.
    let max_tokens = if think { 10240 } else { 3072 };
    let body = serde_json::json!({
        "model": "local",
        "messages": [{ "role": "user", "content": prompt }],
        "stream": true,
        // Gemma is a thinking model; whether it reasons is decided per call by
        // the caller. Reasoning adds depth on multi-step questions but dominates
        // the latency (measured: the same deck analysis took ~47s with thinking
        // vs ~19s without), so callers that don't need it ask for `think=false`
        // and the answer starts streaming almost at once. (`reasoning_budget: 0`
        // did NOT reliably stop it for this build; `enable_thinking` does.)
        "chat_template_kwargs": { "enable_thinking": think },
        "max_tokens": max_tokens
    });
    let resp = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("AI request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("AI engine returned status {}", resp.status()));
    }

    // Parse the Server-Sent Events stream line by line. Chunks may split a
    // line, so we buffer and only process complete lines (ending in '\n').
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("AI stream error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..=pos).collect();
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            let delta = &json["choices"][0]["delta"];
            for (field, kind) in [("reasoning_content", "reasoning"), ("content", "content")] {
                if let Some(t) = delta[field].as_str() {
                    if !t.is_empty() {
                        let _ = app.emit(
                            "ai-delta",
                            AiDelta {
                                kind: kind.to_string(),
                                text: t.to_string(),
                            },
                        );
                    }
                }
            }
        }
    }
    let _ = app.emit("ai-done", ());
    Ok(())
}

/// Hard cap on model⇄tool rounds: a misbehaving model must never loop forever
/// (a bounded loop is mandatory for any agentic tool-calling flow).
const MAX_TOOL_ROUNDS: usize = 6;

/// Runs a tool-calling conversation. Sends `messages` plus the `tools` schema to
/// the model; whenever the model asks for a tool, runs `exec_tool(name, args)`
/// and feeds the result back, looping until the model produces a final answer,
/// which is emitted via the usual `ai-delta`/`ai-done` events. Each requested
/// tool is also surfaced as an `ai-tool` event so the UI can show activity.
///
/// Safeguards (agentic-loop best practices): a round cap, and a debounce that
/// blocks a tool call repeated identically too many times so the model is nudged
/// to change approach instead of spinning.
pub async fn chat_with_tools<F>(
    app: &AppHandle,
    mut messages: Vec<serde_json::Value>,
    tools: serde_json::Value,
    think: bool,
    exec_tool: F,
) -> Result<(), String>
where
    F: Fn(&str, &str) -> Result<String, String>,
{
    let base = ensure_running(app).await?;
    let client = reqwest::Client::new();
    let max_tokens = if think { 10240 } else { 3072 };
    let mut recent: Vec<String> = Vec::new();

    for _round in 0..MAX_TOOL_ROUNDS {
        let body = serde_json::json!({
            "model": "local",
            "messages": messages,
            "tools": tools,
            "stream": false,
            "chat_template_kwargs": { "enable_thinking": think },
            "max_tokens": max_tokens
        });
        let resp = client
            .post(format!("{base}/v1/chat/completions"))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("AI request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("AI engine returned status {}", resp.status()));
        }
        let json: serde_json::Value =
            resp.json().await.map_err(|e| format!("AI parse error: {e}"))?;
        let message = &json["choices"][0]["message"];

        // The model asked for one or more tools: run each, append the results,
        // and loop so it can use them to answer.
        if let Some(calls) = message["tool_calls"].as_array() {
            if !calls.is_empty() {
                messages.push(message.clone());
                for call in calls {
                    let name = call["function"]["name"].as_str().unwrap_or_default();
                    let args = call["function"]["arguments"].as_str().unwrap_or("{}");
                    let id = call["id"].as_str().unwrap_or_default();
                    let _ = app.emit(
                        "ai-tool",
                        serde_json::json!({ "name": name, "arguments": args }),
                    );

                    let signature = format!("{name}({args})");
                    let duplicate = recent.iter().filter(|s| *s == &signature).count() >= 2;
                    recent.push(signature);

                    let result = if duplicate {
                        "BLOCCATO: hai già ripetuto questa stessa chiamata; cambia approccio o concludi con i dati che hai.".to_string()
                    } else {
                        exec_tool(name, args).unwrap_or_else(|e| {
                            // Keep only the last line of an error, so a verbose
                            // failure doesn't flood the model's context.
                            format!("ERRORE tool: {}", e.lines().last().unwrap_or(e.as_str()))
                        })
                    };
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": result
                    }));
                }
                continue;
            }
        }

        // No tool requested: this is the final answer.
        if let Some(content) = message["content"].as_str() {
            if !content.is_empty() {
                let _ = app.emit(
                    "ai-delta",
                    AiDelta { kind: "content".to_string(), text: content.to_string() },
                );
            }
        }
        let _ = app.emit("ai-done", ());
        return Ok(());
    }

    // Reached the round cap without a final answer.
    let _ = app.emit(
        "ai-delta",
        AiDelta {
            kind: "content".to_string(),
            text: "(Analisi interrotta: troppe ricerche consecutive.)".to_string(),
        },
    );
    let _ = app.emit("ai-done", ());
    Ok(())
}

/// Stops the sidecar if it is running (best effort).
pub fn stop() {
    if let Ok(mut guard) = slot().lock() {
        if let Some(mut running) = guard.take() {
            let _ = running.child.kill();
        }
    }
}
