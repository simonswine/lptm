//! In-process LSP client for the LogQL language server.
//!
//! Spawns the `logql-lsp` server in-process using `tokio::io::duplex()` as
//! transport and speaks JSON-RPC 2.0 / LSP over the channel pair.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use log::warn;
use serde_json::{json, Value};
use shared::LokiDiagnosticView;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};
use tower_lsp::{LspService, Server};

use logql_lsp::LogQLBackend;

/// Incoming notification from the server to the client.
#[derive(Debug, Clone)]
pub enum LspNotification {
    Diagnostics {
        diagnostics: Vec<LokiDiagnosticView>,
    },
}

/// Minimal JSON-RPC 2.0 / LSP client.
///
/// Communicates with an in-process `LogQLBackend` over a `tokio::io::duplex`
/// pair so no OS sockets or processes are involved.
pub struct LspClient {
    /// Serialised writes go here (protected by a mutex to serialise concurrent
    /// callers writing to the same stream).
    writer: Arc<Mutex<tokio::io::DuplexStream>>,
    next_id: AtomicI64,
    /// Pending request futures keyed by request id.
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    /// Sender for server-push notifications (kept to hold the channel open).
    #[allow(dead_code)]
    notif_tx: mpsc::UnboundedSender<LspNotification>,
}

impl LspClient {
    /// Create a new `LspClient` and spawn the `LogQLBackend` server in the
    /// background.  Returns after the LSP `initialize` / `initialized`
    /// handshake completes.
    ///
    /// The returned `UnboundedReceiver` carries server-push notifications
    /// (e.g. `textDocument/publishDiagnostics`) and is intentionally kept
    /// separate so it can be used directly in a `tokio::select!` loop.
    pub async fn new() -> Result<(Self, mpsc::UnboundedReceiver<LspNotification>)> {
        // Two duplex streams: one for each direction.
        // client writes to `client_to_server_w`, server reads from `client_to_server_r`
        // server writes to `server_to_client_w`, client reads from `server_to_client_r`
        let (client_to_server_r, client_to_server_w) = tokio::io::duplex(65_536);
        let (server_to_client_r, server_to_client_w) = tokio::io::duplex(65_536);

        // Spawn the tower-lsp server.
        let (service, socket) = LspService::build(LogQLBackend::new)
            .custom_method("loki/labelsUpdate", LogQLBackend::handle_labels_update)
            .custom_method(
                "loki/labelValuesUpdate",
                LogQLBackend::handle_label_values_update,
            )
            .finish();
        tokio::spawn(Server::new(client_to_server_r, server_to_client_w, socket).serve(service));

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (notif_tx, notif_rx) = mpsc::unbounded_channel::<LspNotification>();

        // Spawn reader task.
        let pending_clone = Arc::clone(&pending);
        let notif_tx_clone = notif_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = read_loop(server_to_client_r, pending_clone, notif_tx_clone).await {
                warn!("LSP read loop ended: {e}");
            }
        });

        let client = LspClient {
            writer: Arc::new(Mutex::new(client_to_server_w)),
            next_id: AtomicI64::new(1),
            pending,
            notif_tx,
        };

        // Perform LSP initialization handshake.
        client.initialize().await?;
        client.send_notification("initialized", json!({})).await?;

        Ok((client, notif_rx))
    }

    // ── LSP lifecycle ─────────────────────────────────────────────────────────

    async fn initialize(&self) -> Result<()> {
        let _resp = self
            .send_request(
                "initialize",
                json!({
                    "processId": null,
                    "clientInfo": { "name": "lptm" },
                    "rootUri": null,
                    "capabilities": {
                        "textDocument": {
                            "completion": {
                                "completionItem": { "documentationFormat": ["plaintext"] }
                            },
                            "publishDiagnostics": {}
                        }
                    }
                }),
            )
            .await?;
        Ok(())
    }

    // ── Document sync ─────────────────────────────────────────────────────────

    pub async fn did_open(&self, uri: &str, text: &str) {
        let _ = self
            .send_notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "logql",
                        "version": 1,
                        "text": text
                    }
                }),
            )
            .await;
    }

    pub async fn did_change(&self, uri: &str, text: &str, version: i32) {
        let _ = self
            .send_notification(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": text }]
                }),
            )
            .await;
    }

    // ── Completions ───────────────────────────────────────────────────────────

    pub async fn completion(&self, uri: &str, line: u32, character: u32) -> Vec<CompletionItem> {
        let result = self
            .send_request(
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character }
                }),
            )
            .await;

        match result {
            Ok(resp) => parse_completion_response(resp),
            Err(e) => {
                warn!("completion request failed: {e}");
                vec![]
            }
        }
    }

    // ── Label data injection ──────────────────────────────────────────────────

    pub async fn push_labels(&self, selector: &str, names: Vec<String>) {
        let _ = self
            .send_notification(
                "loki/labelsUpdate",
                json!({ "selector": selector, "names": names }),
            )
            .await;
    }

    pub async fn push_label_values(&self, label: &str, selector: &str, values: Vec<String>) {
        let _ = self
            .send_notification(
                "loki/labelValuesUpdate",
                json!({ "label": label, "selector": selector, "values": values }),
            )
            .await;
    }

    // ── JSON-RPC transport ────────────────────────────────────────────────────

    async fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        write_lsp_message(&mut *self.writer.lock().await, &msg).await?;

        rx.await.context("LSP server dropped connection")
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        write_lsp_message(&mut *self.writer.lock().await, &msg).await
    }
}

// ── Completion item type ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    /// Icon character for the kind.
    pub kind_icon: &'static str,
    /// The replacement range in the query text (byte offsets, start..end).
    pub replace_start_char: u32,
    pub replace_end_char: u32,
    /// The text to insert.
    pub insert_text: String,
}

/// Parse the LSP CompletionResponse (Array or List) into our `CompletionItem`s.
fn parse_completion_response(val: Value) -> Vec<CompletionItem> {
    // CompletionResponse can be `null`, an array, or `{isIncomplete, items}`.
    let items_val = if val.is_array() {
        val
    } else if let Some(items) = val.get("items") {
        items.clone()
    } else {
        return vec![];
    };

    let items_arr = match items_val.as_array() {
        Some(a) => a,
        None => return vec![],
    };

    items_arr
        .iter()
        .filter_map(|item| {
            let label = item.get("label")?.as_str()?.to_string();
            let detail = item
                .get("detail")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let documentation = item.get("documentation").and_then(|v| {
                if v.is_string() {
                    v.as_str().map(str::to_string)
                } else {
                    v.get("value").and_then(|v| v.as_str()).map(str::to_string)
                }
            });

            let kind_num = item.get("kind").and_then(|v| v.as_u64()).unwrap_or(0);
            let kind_icon = lsp_kind_icon(kind_num);

            // text_edit carries the replacement range (in LSP line/character offsets).
            let (replace_start_char, replace_end_char, insert_text) =
                if let Some(te) = item.get("textEdit") {
                    let start = te
                        .get("range")
                        .and_then(|r| r.get("start"))
                        .and_then(|s| s.get("character"))
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0) as u32;
                    let end = te
                        .get("range")
                        .and_then(|r| r.get("end"))
                        .and_then(|s| s.get("character"))
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0) as u32;
                    let text = te
                        .get("newText")
                        .and_then(|t| t.as_str())
                        .unwrap_or(&label)
                        .to_string();
                    (start, end, text)
                } else {
                    (0, 0, label.clone())
                };

            Some(CompletionItem {
                label,
                detail,
                documentation,
                kind_icon,
                replace_start_char,
                replace_end_char,
                insert_text,
            })
        })
        .collect()
}

fn lsp_kind_icon(kind: u64) -> &'static str {
    match kind {
        1 => " T",  // Text
        2 => " ƒ",  // Method
        3 => " ƒ",  // Function
        4 => " ƒ",  // Constructor
        5 => " ·",  // Field
        6 => " x",  // Variable
        7 => " ·",  // Class
        8 => " ·",  // Interface
        9 => " ·",  // Module
        10 => " ·", // Property
        11 => " U", // Unit
        12 => " V", // Value
        13 => " ·", // Enum
        14 => " ⌨", // Keyword
        15 => " ·", // Snippet
        16 => " ·", // Color
        17 => " ·", // File
        18 => " ·", // Reference
        19 => " ·", // Folder
        20 => " ·", // EnumMember
        21 => " C", // Constant
        22 => " ·", // Struct
        23 => " ·", // Event
        24 => " ·", // Operator
        25 => " ·", // TypeParameter
        _ => " ·",
    }
}

// ── LSP wire format helpers ───────────────────────────────────────────────────

/// Write a single JSON-RPC message with `Content-Length` framing.
async fn write_lsp_message(w: &mut tokio::io::DuplexStream, msg: &Value) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    w.write_all(header.as_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

/// Background task: read LSP messages from the server and dispatch them.
async fn read_loop(
    reader: tokio::io::DuplexStream,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    notif_tx: mpsc::UnboundedSender<LspNotification>,
) -> Result<()> {
    let mut buf = BufReader::new(reader);

    loop {
        // Read headers.
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = buf.read_line(&mut line).await?;
            if n == 0 {
                return Ok(()); // EOF — server closed
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break; // blank line = end of headers
            }
            if let Some(val) = line.strip_prefix("Content-Length: ") {
                content_length = val.trim().parse().ok();
            }
        }

        let len = content_length.ok_or_else(|| anyhow!("missing Content-Length"))?;
        let mut body = vec![0u8; len];
        tokio::io::AsyncReadExt::read_exact(&mut buf, &mut body).await?;

        let msg: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                warn!("failed to parse LSP message: {e}");
                continue;
            }
        };

        dispatch_message(msg, &pending, &notif_tx).await;
    }
}

async fn dispatch_message(
    msg: Value,
    pending: &Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    notif_tx: &mpsc::UnboundedSender<LspNotification>,
) {
    let method = msg
        .get("method")
        .and_then(|m| m.as_str())
        .map(str::to_string);

    if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
        // It's a response to one of our requests.
        let result = msg.get("result").cloned().unwrap_or(Value::Null);
        if let Some(tx) = pending.lock().await.remove(&id) {
            let _ = tx.send(result);
        }
        return;
    }

    // It's a notification from the server.
    if let Some(method) = method {
        if method == "textDocument/publishDiagnostics" {
            if let Some(params) = msg.get("params") {
                let diags = parse_lsp_diagnostics(params);
                let _ = notif_tx.send(LspNotification::Diagnostics { diagnostics: diags });
            }
        }
    }
}

fn parse_lsp_diagnostics(params: &Value) -> Vec<LokiDiagnosticView> {
    let arr = match params.get("diagnostics").and_then(|d| d.as_array()) {
        Some(a) => a,
        None => return vec![],
    };
    arr.iter()
        .filter_map(|d| {
            let message = d.get("message")?.as_str()?.to_string();
            let severity = match d.get("severity").and_then(|s| s.as_u64()).unwrap_or(1) {
                1 => "error",
                2 => "warning",
                _ => "info",
            }
            .to_string();
            let range = d.get("range")?;
            let start = range.get("start")?;
            let end_r = range.get("end")?;
            // LSP gives line/character; we store as byte offsets.
            // For single-line queries line is always 0 so character == byte roughly.
            let start_byte = start.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as usize;
            let end_byte = end_r.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as usize;
            Some(LokiDiagnosticView {
                severity,
                message,
                start_byte,
                end_byte,
            })
        })
        .collect()
}
