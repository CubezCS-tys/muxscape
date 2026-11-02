//! Tauri shell: hosts the Rust core and bridges it to the webview.
//!
//! * `mux_request` — async request/response (everything structural).
//! * `mux_send` — synchronous, ordered fast path for keystrokes/resizes.
//! * `mux-event` — every core event, pushed to the frontend.
//!
//! Quitting the app just drops the SSH connections: tmux keeps every
//! session and process running on the servers.

use mux_core::api::Request;
use mux_core::hosts::HostStore;
use mux_core::ssh_config::SshConfig;
use mux_core::core::VaultConfig;
use mux_core::vault::KdfParams;
use mux_core::Core;
use mux_ssh::KnownHosts;
use tauri::{Emitter, Manager, State};

struct AppCore(Core);

#[tauri::command]
async fn mux_request(core: State<'_, AppCore>, req: Request) -> Result<serde_json::Value, String> {
    core.0.handle(req).await
}

#[tauri::command]
fn mux_send(core: State<'_, AppCore>, req: Request) -> Result<(), String> {
    match core.0.handle_now(&req) {
        Some(r) => r.map(|_| ()),
        None => Err("mux_send only accepts input/resize requests".into()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        // Opens forwarded ports / links in the user's browser.
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            let store = HostStore::open(Some(config_dir.join("hosts.json")), SshConfig::load_default());
            // Share known_hosts with OpenSSH so trust decisions carry over.
            let known_hosts = KnownHosts::openssh_default()
                .unwrap_or_else(|| KnownHosts { path: config_dir.join("known_hosts") });
            // Encrypted password vault, next to hosts.json.
            let vault = VaultConfig { path: Some(config_dir.join("vault.json")), kdf: KdfParams::DEFAULT };
            let (core, mut events) =
                tauri::async_runtime::block_on(async move { Core::with_vault(store, known_hosts, vault) });
            app.manage(AppCore(core));
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Some(ev) = events.recv().await {
                    let _ = handle.emit("mux-event", &ev);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![mux_request, mux_send])
        .run(tauri::generate_context!())
        .expect("error while running Muxscape");
}
