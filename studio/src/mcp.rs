//! MCP server: lets Claude (the built-in assistant, or any MCP client) edit the
//! open project. Requests arrive on HTTP threads and are handed to the UI
//! thread as `Call`s; the editor answers through each call's reply channel.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use iced::Subscription;
use iced::futures::SinkExt;
use serde_json::{Value, json};

pub const SERVER_KEY: &str = "ai-video-editor";
/// Header the built-in assistant sends with every request, naming its run (or `?run=` in the URL).
pub const RUN_HEADER: &str = "X-Aive-Run";
const DEFAULT_PORT: u16 = 47822;
const CALL_TIMEOUT: Duration = Duration::from_secs(180);

/// MCP content blocks, or an error message for the model.
pub type Reply = Result<Vec<Value>, String>;

#[derive(Debug, Clone)]
pub struct Call {
    pub name: String,
    pub args: Value,
    pub reply: Sender<Reply>,
    /// The built-in assistant run that made the call (`None` for other MCP clients).
    pub run: Option<String>,
}

impl Call {
    pub fn answer(&self, reply: Reply) {
        let _ = self.reply.send(reply);
    }
}

pub fn text(s: impl Into<String>) -> Vec<Value> {
    vec![json!({ "type": "text", "text": s.into() })]
}

pub fn image_jpeg(bytes: &[u8], caption: &str) -> Vec<Value> {
    use base64::Engine;
    vec![
        json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(bytes), "mimeType": "image/jpeg" }),
        json!({ "type": "text", "text": caption }),
    ]
}

static CALLS: OnceLock<Mutex<Option<Receiver<Call>>>> = OnceLock::new();
static PORT: AtomicU16 = AtomicU16::new(0);

/// URL of the running server.
pub fn url() -> String {
    format!("http://127.0.0.1:{}/mcp", PORT.load(Ordering::Relaxed))
}

/// Starts the server on the first free port from 47822 (or `AIVE_STUDIO_MCP_PORT`).
pub fn start() -> anyhow::Result<()> {
    let base = std::env::var("AIVE_STUDIO_MCP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
    let (server, port) = (base..base.saturating_add(20))
        .find_map(|p| tiny_http::Server::http(("127.0.0.1", p)).ok().map(|s| (s, p)))
        .ok_or_else(|| anyhow::anyhow!("could not open a local port for the assistant"))?;
    PORT.store(port, Ordering::Relaxed);
    let (tx, rx) = channel();
    *CALLS.get_or_init(Default::default).lock().unwrap() = Some(rx);
    std::thread::Builder::new().name("mcp-http".into()).spawn(move || {
        for request in server.incoming_requests() {
            let tx = tx.clone();
            // Each request on its own thread, so a slow frame render never blocks others.
            std::thread::spawn(move || handle_http(tx, request));
        }
    })?;
    Ok(())
}

/// Calls from the server, delivered to the app.
pub fn subscription() -> Subscription<Call> {
    Subscription::run(|| {
        iced::stream::channel(64, async |mut out: iced::futures::channel::mpsc::Sender<Call>| {
            let Some(rx) = CALLS.get().and_then(|c| c.lock().unwrap().take()) else { return };
            loop {
                match rx.try_recv() {
                    Ok(call) => {
                        let _ = out.send(call).await;
                    }
                    Err(TryRecvError::Empty) => tokio::time::sleep(Duration::from_millis(15)).await,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        })
    })
}

fn respond(req: tiny_http::Request, status: u16, body: Option<String>) {
    let mut resp = tiny_http::Response::from_string(body.unwrap_or_default()).with_status_code(status);
    if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]) {
        resp.add_header(h);
    }
    let _ = req.respond(resp);
}

fn handle_http(tx: Sender<Call>, mut req: tiny_http::Request) {
    // Only local pages may talk to us (protects against DNS rebinding from websites).
    let origin = req.headers().iter().find(|h| h.field.equiv("Origin")).map(|h| h.value.as_str().to_string());
    // The built-in assistant names its run in a header (Claude Code) or in the URL (`/mcp?run=…`, Codex).
    let run = req
        .headers()
        .iter()
        .find(|h| h.field.equiv(RUN_HEADER))
        .map(|h| h.value.as_str().to_string())
        .or_else(|| req.url().split_once("?run=").map(|(_, r)| r.split('&').next().unwrap_or(r).to_string()));
    if let Some(o) = origin {
        let local = o.starts_with("http://127.0.0.1") || o.starts_with("http://localhost") || o == "null";
        if !local {
            return respond(req, 403, Some(r#"{"error":"forbidden origin"}"#.into()));
        }
    }
    if req.method() != &tiny_http::Method::Post || !req.url().starts_with("/mcp") {
        return respond(req, 404, None);
    }
    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        return respond(req, 400, None);
    }
    match serde_json::from_str::<Value>(&body) {
        Ok(msg) => match handle_message(&tx, msg, run) {
            Some(reply) => respond(req, 200, Some(reply.to_string())),
            None => respond(req, 202, None),
        },
        Err(_) => respond(req, 400, Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }).to_string())),
    }
}

fn handle_message(tx: &Sender<Call>, msg: Value, run: Option<String>) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": SERVER_KEY, "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": crate::editor::tools::schemas() })),
        "tools/call" => {
            let (reply_tx, reply_rx) = channel();
            let call = Call { name: msg["params"]["name"].as_str().unwrap_or("").to_string(), args: msg["params"]["arguments"].clone(), reply: reply_tx, run };
            let outcome = if tx.send(call).is_err() { Err("The editor isn't running".to_string()) } else { reply_rx.recv_timeout(CALL_TIMEOUT).unwrap_or_else(|_| Err("The editor took too long to answer".into())) };
            Ok(match outcome {
                Ok(content) => json!({ "content": content, "isError": false }),
                Err(e) => json!({ "content": text(e), "isError": true }),
            })
        }
        _ => Err(json!({ "code": -32601, "message": format!("Unknown method {method}") })),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
    })
}

const INSTRUCTIONS: &str = "You are editing a video in AI Video Editor, a motion graphics editor the user is watching live. \
Every change appears instantly in their preview and can be undone. Times are in seconds on the timeline. Positions (x, y) are \
fractions of the canvas from the top-left (0.5, 0.5 is the center). Sizes such as font size, corner radius and border width are \
in canvas units: pixels at a 1080-pixel short side. Call get_project first to see clip ids, tracks and the selection.";
