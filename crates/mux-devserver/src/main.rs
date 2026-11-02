//! Development bridge: serves the built UI and exposes the Muxscape core
//! over a WebSocket, so the exact same frontend can run in a normal browser
//! (and in headless browser tests) without the Tauri shell.
//!
//! Security: binds to 127.0.0.1 only and requires a random token, because
//! whoever talks to this socket can use your SSH keys.
//!
//! Env:
//!   MUXSCAPE_PORT          (default 7878)
//!   MUXSCAPE_HOSTS_FILE    saved hosts json (default: in-memory)
//!   MUXSCAPE_SSH_CONFIG    ssh_config to read (default ~/.ssh/config, "none" to disable)
//!   MUXSCAPE_KNOWN_HOSTS   known_hosts file (default ~/.ssh/known_hosts)
//!   MUXSCAPE_TOKEN         fixed token instead of a random one
//!   MUXSCAPE_UI_DIR        static UI directory (default ./ui/dist)

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use mux_core::api::Request;
use mux_core::hosts::HostStore;
use mux_core::ssh_config::SshConfig;
use mux_core::Core;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::broadcast;

#[derive(Clone)]
struct App {
    core: Core,
    events: broadcast::Sender<String>,
    token: String,
}

#[derive(Deserialize)]
struct WireRequest {
    id: u64,
    req: Value,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());

    let ssh_config = match env("MUXSCAPE_SSH_CONFIG").as_deref() {
        Some("none") => SshConfig::default(),
        Some(p) => SshConfig::load(&PathBuf::from(p), &mux_ssh::home_dir().unwrap_or_default()),
        None => SshConfig::load_default(),
    };
    let store = HostStore::open(env("MUXSCAPE_HOSTS_FILE").map(PathBuf::from), ssh_config);
    let known_hosts = match env("MUXSCAPE_KNOWN_HOSTS") {
        Some(p) => mux_ssh::KnownHosts { path: p.into() },
        None => mux_ssh::KnownHosts::openssh_default().expect("no home directory"),
    };
    let (core, mut rx) = Core::new(store, known_hosts);
    let (tx, _) = broadcast::channel::<String>(65536);
    let t = tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = t.send(json!({ "event": ev }).to_string());
        }
    });

    let token = env("MUXSCAPE_TOKEN").unwrap_or_else(|| {
        use rand::Rng;
        rand::rng().sample_iter(&rand::distr::Alphanumeric).take(24).map(char::from).collect()
    });
    let port: u16 = env("MUXSCAPE_PORT").and_then(|p| p.parse().ok()).unwrap_or(7878);
    let ui_dir = env("MUXSCAPE_UI_DIR").unwrap_or_else(|| "ui/dist".into());

    let app = App { core, events: tx, token: token.clone() };
    let router = Router::new()
        .route("/ws", get(ws))
        .fallback_service(tower_http::services::ServeDir::new(ui_dir))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
    println!("Muxscape dev bridge: http://127.0.0.1:{port}/?token={token}");
    axum::serve(listener, router).await.unwrap();
}

async fn ws(State(app): State<App>, Query(q): Query<HashMap<String, String>>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    if q.get("token") != Some(&app.token) {
        return (axum::http::StatusCode::FORBIDDEN, "bad token").into_response();
    }
    upgrade.max_message_size(64 << 20).on_upgrade(move |s| session(app, s))
}

async fn session(app: App, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut events = app.events.subscribe();
    let o = out_tx.clone();
    let fwd = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(e) => {
                    if o.send(e).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!("ws client lagged {n} events"),
                Err(_) => break,
            }
        }
    });
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m.into())).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else { continue };
        let Ok(w) = serde_json::from_str::<WireRequest>(&text) else { continue };
        // Keystrokes/resizes are handled inline, in arrival order.
        if let Ok(r) = serde_json::from_value::<Request>(w.req.clone()) {
            if let Some(res) = app.core.handle_now(&r) {
                let reply = match res {
                    Ok(v) => json!({ "id": w.id, "ok": v }),
                    Err(e) => json!({ "id": w.id, "err": e }),
                };
                let _ = out_tx.send(reply.to_string());
                continue;
            }
        }
        let core = app.core.clone();
        let out = out_tx.clone();
        tokio::spawn(async move {
            let reply = match serde_json::from_value::<Request>(w.req) {
                Ok(r) => match core.handle(r).await {
                    Ok(v) => json!({ "id": w.id, "ok": v }),
                    Err(e) => json!({ "id": w.id, "err": e }),
                },
                Err(e) => json!({ "id": w.id, "err": format!("bad request: {e}") }),
            };
            let _ = out.send(reply.to_string());
        });
    }
    fwd.abort();
    writer.abort();
}
