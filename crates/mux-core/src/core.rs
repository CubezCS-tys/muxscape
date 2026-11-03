//! Connection manager and request router.
//!
//! * One SSH connection per host, shared by everything on that host.
//! * Any number of hosts connected at once.
//! * Each attached tmux session is an [`AttachmentTask`] with a stable id
//!   that survives reconnects.
//! * When SSH drops, the host goes to `Reconnecting`, retries with backoff,
//!   and re-attaches every session it had — the remote tmux sessions (and
//!   every process in them) never noticed.

use crate::api::*;
use crate::attachment::{AttachCmd, AttachmentTask};
use crate::control::ControlClient;
use crate::hosts::{Host, HostStore};
use crate::ports;
use crate::remote;
use crate::vault::{self, KdfParams, Vault, VaultError};
use crate::tunnels::{PortDetector, SavedForward, TunnelId, TunnelInfo};
use base64::Engine;
use mux_ssh::{HostKeyDecision, HostKeyInfo, KnownHosts, Prompter, SshConnection};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use zeroize::Zeroizing;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tmux_control::commands as cmd;
use tmux_control::{SessionId, WindowId};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

type Res = Result<Value, String>;

#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

struct Inner {
    hosts: Mutex<HostStore>,
    known_hosts: KnownHosts,
    ui: mpsc::UnboundedSender<Event>,
    /// Attachments emit here; the router inspects and forwards to `ui`.
    internal: mpsc::UnboundedSender<Event>,
    sessions_changed: mpsc::UnboundedSender<HostId>,
    conns: Mutex<HashMap<HostId, Arc<Conn>>>,
    attachments: Mutex<HashMap<AttachmentId, Att>>,
    shells: Mutex<HashMap<ShellId, Shell>>,
    prompts: Mutex<HashMap<PromptId, oneshot::Sender<PromptAnswer>>>,
    next_id: AtomicU64,
    tunnels: Mutex<HashMap<TunnelId, Tunnel>>,
    /// Serialises tunnel creation so one port never gets two listeners.
    tunnel_open: tokio::sync::Mutex<()>,
    /// Per pane ("a<att>:<pane>") or shell ("s<id>") output scanners.
    detectors: Mutex<HashMap<String, PortDetector>>,
    /// (host, port) already offered to the UI.
    suggested: Mutex<HashSet<(HostId, u16)>>,
    vault: Mutex<Vault>,
    /// Seconds since `epoch` of the last UI request (for auto-lock).
    last_activity: AtomicU64,
    epoch: Instant,
}

/// Where the password vault lives and how hard new master keys are to brute
/// force.
pub struct VaultConfig {
    /// `None`: in memory only (tests, dev server).
    pub path: Option<PathBuf>,
    pub kdf: KdfParams,
}

/// Lock the vault after this long without any UI activity.
pub const VAULT_AUTO_LOCK: Duration = Duration::from_secs(8 * 3600);

struct Tunnel {
    info: TunnelInfo,
    /// Accept loop; owns every connection task (aborting it ends them all).
    task: tokio::task::JoinHandle<()>,
}

struct Conn {
    host_id: HostId,
    ssh: Mutex<Option<Arc<SshConnection>>>,
    state: Mutex<ConnState>,
    /// False once the user asked to disconnect: no auto-reconnect.
    wanted: AtomicBool,
    connecting: tokio::sync::Mutex<()>,
}

struct Att {
    host_id: HostId,
    /// Shown in a pop-out window, attached to a companion session.
    popout: bool,
    tx: mpsc::UnboundedSender<AttachCmd>,
    session: Arc<Mutex<Option<SessionId>>>,
    live: bool,
}

struct Shell {
    host_id: HostId,
    /// Ordered queue to the channel writer task.
    tx: mpsc::UnboundedSender<ShellOp>,
}

enum ShellOp {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| e.to_string())
}

fn b64e(d: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(d)
}

impl Core {
    pub fn new(hosts: HostStore, known_hosts: KnownHosts) -> (Core, mpsc::UnboundedReceiver<Event>) {
        Core::with_vault(hosts, known_hosts, VaultConfig { path: None, kdf: KdfParams::DEFAULT })
    }

    pub fn with_vault(
        hosts: HostStore,
        known_hosts: KnownHosts,
        vault_cfg: VaultConfig,
    ) -> (Core, mpsc::UnboundedReceiver<Event>) {
        let mut vault = Vault::open(vault_cfg.path, vault_cfg.kdf);
        // "Remember on this device": unlock at startup.
        match vault.unlock_remembered() {
            Ok(true) => tracing::info!("vault unlocked from the system credential store"),
            Ok(false) | Err(VaultError::NotSetUp) => {}
            Err(e) => tracing::warn!("vault: {e}"),
        }
        let (ui, ui_rx) = mpsc::unbounded_channel();
        let (internal, mut internal_rx) = mpsc::unbounded_channel::<Event>();
        let (sc_tx, mut sc_rx) = mpsc::unbounded_channel::<HostId>();
        let core = Core {
            inner: Arc::new(Inner {
                hosts: Mutex::new(hosts),
                known_hosts,
                ui,
                internal,
                sessions_changed: sc_tx,
                conns: Mutex::new(HashMap::new()),
                attachments: Mutex::new(HashMap::new()),
                shells: Mutex::new(HashMap::new()),
                prompts: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                tunnels: Mutex::new(HashMap::new()),
                tunnel_open: tokio::sync::Mutex::new(()),
                detectors: Mutex::new(HashMap::new()),
                suggested: Mutex::new(HashSet::new()),
                vault: Mutex::new(vault),
                last_activity: AtomicU64::new(0),
                epoch: Instant::now(),
            }),
        };
        // Auto-lock after a long idle period.
        let weak = Arc::downgrade(&core.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let Some(inner) = weak.upgrade() else { return };
                let c = Core { inner };
                let idle = c.inner.epoch.elapsed().as_secs().saturating_sub(c.inner.last_activity.load(Ordering::Relaxed));
                if idle >= VAULT_AUTO_LOCK.as_secs() && c.inner.vault.lock().unwrap().is_unlocked() {
                    c.inner.vault.lock().unwrap().lock();
                    tracing::info!("vault auto-locked after {idle}s idle");
                    c.emit_vault();
                }
            }
        });
        // Router: watch attachment events before handing them to the UI.
        let c = core.clone();
        tokio::spawn(async move {
            while let Some(ev) = internal_rx.recv().await {
                c.observe(&ev);
                let _ = c.inner.ui.send(ev);
            }
        });
        // Session list refreshes, debounced per host.
        let c = core.clone();
        tokio::spawn(async move {
            while let Some(h) = sc_rx.recv().await {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let mut hosts = vec![h];
                while let Ok(h) = sc_rx.try_recv() {
                    if !hosts.contains(&h) {
                        hosts.push(h);
                    }
                }
                for h in hosts {
                    let _ = c.discover(&h).await;
                }
            }
        });
        (core, ui_rx)
    }

    fn emit(&self, e: Event) {
        let _ = self.inner.ui.send(e);
    }

    fn id(&self) -> u64 {
        self.inner.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn observe(&self, ev: &Event) {
        match ev {
            Event::AttachmentStatus { attachment, live } => {
                let host = {
                    let mut atts = self.inner.attachments.lock().unwrap();
                    let Some(a) = atts.get_mut(attachment) else { return };
                    a.live = *live;
                    a.host_id.clone()
                };
                if !*live {
                    // Control channel ended without %exit. If SSH itself is
                    // fine (e.g. tmux crashed, or attach failed) try to
                    // recover now; otherwise the reconnect loop handles it.
                    let c = self.clone();
                    let id = *attachment;
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        if let Some(ssh) = c.current_ssh(&host) {
                            if !ssh.is_closed() {
                                c.reattach(&host, &ssh, &[id]).await;
                            }
                        }
                    });
                }
            }
            Event::AttachmentClosed { attachment, .. } => {
                self.inner.attachments.lock().unwrap().remove(attachment);
                let prefix = format!("a{attachment}:");
                self.inner.detectors.lock().unwrap().retain(|k, _| !k.starts_with(&prefix));
            }
            Event::PaneOutput { attachment, pane, data } => {
                let host = self.inner.attachments.lock().unwrap().get(attachment).map(|a| a.host_id.clone());
                if let (Some(host), Ok(bytes)) = (host, b64d(data)) {
                    self.scan_output(&host, format!("a{attachment}:{pane}"), &bytes);
                }
            }
            _ => {}
        }
    }

    fn conn(&self, host_id: &str) -> Arc<Conn> {
        self.inner
            .conns
            .lock()
            .unwrap()
            .entry(host_id.to_string())
            .or_insert_with(|| {
                Arc::new(Conn {
                    host_id: host_id.to_string(),
                    ssh: Mutex::new(None),
                    state: Mutex::new(ConnState::Disconnected),
                    wanted: AtomicBool::new(false),
                    connecting: tokio::sync::Mutex::new(()),
                })
            })
            .clone()
    }

    fn current_ssh(&self, host_id: &str) -> Option<Arc<SshConnection>> {
        self.inner.conns.lock().unwrap().get(host_id)?.ssh.lock().unwrap().clone()
    }

    fn set_state(&self, conn: &Conn, state: ConnState, message: Option<String>) {
        *conn.state.lock().unwrap() = state.clone();
        self.emit(Event::Connection { host_id: conn.host_id.clone(), state, message });
    }

    fn host(&self, host_id: &str) -> Result<Host, String> {
        self.inner.hosts.lock().unwrap().get(host_id).ok_or_else(|| format!("unknown host {host_id}"))
    }

    fn emit_hosts(&self) {
        let h = self.inner.hosts.lock().unwrap();
        self.emit(Event::Hosts { hosts: h.list(), folders: h.folders() });
    }

    // ------------------------------------------------------------ requests

    fn touch(&self) {
        self.inner.last_activity.store(self.inner.epoch.elapsed().as_secs(), Ordering::Relaxed);
    }

    pub async fn handle(&self, req: Request) -> Res {
        use Request as R;
        // Background polling (Ports panel, session refresh) is not "using
        // the app": it must not keep the vault from auto-locking.
        let background = matches!(
            req,
            R::ListHosts | R::ListTunnels | R::ScanPorts { .. } | R::ProbePorts { .. } | R::VaultStatus
                | R::RefreshSessions { .. } | R::RefreshPane { .. } | R::Resize { .. } | R::ShellResize { .. }
        );
        if !background {
            self.touch();
        }
        match req {
            R::ListHosts => {
                let h = self.inner.hosts.lock().unwrap();
                let conns = self.inner.conns.lock().unwrap();
                let states: HashMap<&String, ConnState> =
                    conns.iter().map(|(k, c)| (k, c.state.lock().unwrap().clone())).collect();
                Ok(json!({ "hosts": h.list(), "folders": h.folders(), "states": states }))
            }
            R::SaveHost { host } => {
                let saved = self.inner.hosts.lock().unwrap().upsert(host).map_err(|e| e.to_string())?;
                self.emit_hosts();
                Ok(json!(saved))
            }
            R::DeleteHost { host_id } => {
                self.disconnect(&host_id).await;
                // Its saved password goes with it (unless another host
                // reaches the same user@host:port).
                let slot = self.host(&host_id).ok().and_then(|h| {
                    let hs = self.inner.hosts.lock().unwrap();
                    let target = hs.target(&h).ok()?.display_chain();
                    let shared = hs.list().iter().any(|o| {
                        o.id != host_id && hs.target(o).map(|t| t.display_chain() == target).unwrap_or(false)
                    });
                    (!shared).then(|| vault::password_slot(&target))
                });
                if let Some(slot) = slot {
                    if self.inner.vault.lock().unwrap().remove(&slot).unwrap_or(false) {
                        self.emit_vault();
                    }
                }
                self.inner.hosts.lock().unwrap().remove(&host_id).map_err(|e| e.to_string())?;
                self.emit_hosts();
                Ok(Value::Null)
            }
            R::AddFolder { folder } => {
                self.inner.hosts.lock().unwrap().add_folder(&folder).map_err(|e| e.to_string())?;
                self.emit_hosts();
                Ok(Value::Null)
            }
            R::Connect { host_id } => {
                self.ensure_connected(&host_id).await?;
                let d = self.discover(&host_id).await?;
                Ok(json!({ "tmux": d.tmux, "sessions": d.sessions }))
            }
            R::Disconnect { host_id } => {
                self.disconnect(&host_id).await;
                Ok(Value::Null)
            }
            R::RefreshSessions { host_id } => {
                let d = self.discover(&host_id).await?;
                Ok(json!({ "tmux": d.tmux, "sessions": d.sessions }))
            }
            R::Attach { host_id, target, cols, rows } => {
                let id = self.attach(&host_id, target, cols, rows).await?;
                Ok(json!({ "attachment": id }))
            }
            R::PopOut { host_id, session, window, cols, rows } => {
                let (attachment, session) = self.pop_out(&host_id, session, window, cols, rows).await?;
                Ok(json!({ "attachment": attachment, "session": session }))
            }
            R::ClosePopout { attachment } => {
                self.close_popout(attachment).await?;
                Ok(Value::Null)
            }
            R::Detach { attachment } => {
                self.att_send(attachment, AttachCmd::Detach)?;
                Ok(Value::Null)
            }
            R::KillSession { host_id, session } => {
                // Kill through an exec so it works for unattached sessions too.
                let ssh = self.ensure_connected(&host_id).await?;
                let out = ssh.exec(&remote::kill_session_command(session)).await.map_err(|e| e.to_string())?;
                if !out.success() {
                    return Err(format!("kill-session failed: {}", out.stderr_str().trim()));
                }
                let _ = self.discover(&host_id).await;
                Ok(Value::Null)
            }
            R::RenameSession { attachment, name } => {
                let sid = self.att_session(attachment)?;
                self.att_command(attachment, cmd::rename_session(sid, &name)).await
            }
            R::SelectWindow { attachment, window } => {
                let sid = self.att_session(attachment)?;
                self.att_command(attachment, cmd::select_window(sid, window)).await
            }
            R::NewWindow { attachment } => {
                let sid = self.att_session(attachment)?;
                self.att_command(attachment, cmd::new_window(sid, None, None)).await
            }
            R::KillWindow { attachment, window } => self.att_command(attachment, cmd::kill_window(window)).await,
            R::RenameWindow { attachment, window, name } => {
                self.att_command(attachment, cmd::rename_window(window, &name)).await
            }
            R::SelectPane { attachment, pane } => self.att_command(attachment, cmd::select_pane(pane)).await,
            R::SplitPane { attachment, pane, horizontal } => {
                self.att_command(attachment, cmd::split_pane(pane, horizontal)).await
            }
            R::KillPane { attachment, pane } => self.att_command(attachment, cmd::kill_pane(pane)).await,
            R::ZoomPane { attachment, pane } => self.att_command(attachment, cmd::toggle_zoom(pane)).await,
            R::MoveDivider { attachment, window, path, index, delta } => {
                let (tx, rx) = oneshot::channel();
                self.att_send(attachment, AttachCmd::MoveDivider { window, path, index, delta, reply: tx })?;
                rx.await.map_err(|_| "attachment closed".to_string())?.map(|_| Value::Null)
            }
            r @ (R::Input { .. } | R::Resize { .. } | R::ShellInput { .. } | R::ShellResize { .. }) => {
                self.handle_now(&r).unwrap_or(Ok(Value::Null))
            }
            R::RefreshPane { attachment, pane } => {
                self.att_send(attachment, AttachCmd::RefreshPane { pane })?;
                Ok(Value::Null)
            }
            R::OpenShell { host_id, cols, rows } => {
                let id = self.open_shell(&host_id, cols, rows).await?;
                Ok(json!({ "shell": id }))
            }
            R::CloseShell { shell } => {
                if let Some(s) = self.inner.shells.lock().unwrap().remove(&shell) {
                    let _ = s.tx.send(ShellOp::Close);
                }
                Ok(Value::Null)
            }
            R::ListTunnels => Ok(json!(self.tunnel_list())),
            R::OpenTunnel { host_id, remote_port, local_port, path, pinned } => {
                let info = self.open_tunnel(&host_id, remote_port, local_port, path, pinned).await?;
                Ok(json!(info))
            }
            R::CloseTunnel { tunnel } => {
                self.close_tunnel(tunnel, true);
                Ok(Value::Null)
            }
            R::PinTunnel { tunnel, pinned } => {
                self.pin_tunnel(tunnel, pinned)?;
                Ok(Value::Null)
            }
            R::ScanPorts { host_id } => {
                let ssh = self.ensure_connected(&host_id).await?;
                let out = ssh.exec(&ports::scan_command()).await.map_err(|e| e.to_string())?;
                Ok(json!({ "ports": ports::parse_scan(&out.stdout_str()) }))
            }
            R::ProbePorts { host_id, targets } => {
                let ssh = self.ensure_connected(&host_id).await?;
                // A few at a time: each is one SSH channel.
                let limit = Arc::new(tokio::sync::Semaphore::new(8));
                let mut set = JoinSet::new();
                for t in targets.into_iter().take(200) {
                    let (ssh, limit) = (ssh.clone(), limit.clone());
                    set.spawn(async move {
                        let _permit = limit.acquire_owned().await;
                        ports::probe(&ssh, t.port, t.process.as_deref()).await
                    });
                }
                let mut probes = Vec::new();
                while let Some(r) = set.join_next().await {
                    if let Ok(p) = r {
                        probes.push(p);
                    }
                }
                probes.sort_by_key(|p| p.port);
                Ok(json!({ "probes": probes }))
            }
            R::VaultStatus => Ok(json!(self.inner.vault.lock().unwrap().status())),
            R::VaultCreate { password, remember } => {
                let pw = Zeroizing::new(password.0);
                self.vault_blocking(move |v| v.create(&pw, remember)).await?;
                Ok(Value::Null)
            }
            R::VaultUnlock { password, remember } => {
                let pw = Zeroizing::new(password.0);
                self.vault_blocking(move |v| {
                    v.unlock(&pw)?;
                    if remember {
                        v.set_remember(true)?;
                    }
                    Ok(())
                })
                .await?;
                Ok(Value::Null)
            }
            R::VaultLock => {
                self.inner.vault.lock().unwrap().lock();
                self.emit_vault();
                Ok(Value::Null)
            }
            R::VaultChangePassword { old, new } => {
                let (old, new) = (Zeroizing::new(old.0), Zeroizing::new(new.0));
                self.vault_blocking(move |v| v.change_password(&old, &new)).await?;
                Ok(Value::Null)
            }
            R::VaultSetRemember { remember } => {
                self.vault_blocking(move |v| v.set_remember(remember)).await?;
                Ok(Value::Null)
            }
            R::VaultForget { slot } => {
                self.vault_blocking(move |v| v.remove(&slot).map(|_| ())).await?;
                Ok(Value::Null)
            }
            R::VaultReset => {
                self.vault_blocking(|v| v.reset()).await?;
                Ok(Value::Null)
            }
            R::AnswerPrompt { prompt, answer } => {
                if let Some(tx) = self.inner.prompts.lock().unwrap().remove(&prompt) {
                    let _ = tx.send(answer);
                }
                Ok(Value::Null)
            }
        }
    }

    /// Order-sensitive requests (keystrokes, resizes), handled without
    /// awaiting so callers can process them strictly in arrival order.
    /// Returns `None` for requests that need [`Core::handle`].
    pub fn handle_now(&self, req: &Request) -> Option<Res> {
        use Request as R;
        if matches!(req, R::Input { .. } | R::ShellInput { .. }) {
            self.touch();
        }
        let r = match req {
            R::Input { attachment, pane, data } => b64d(data)
                .and_then(|d| self.att_send(*attachment, AttachCmd::Input { pane: *pane, data: d })),
            R::Resize { attachment, cols, rows } => {
                self.att_send(*attachment, AttachCmd::Resize { cols: *cols, rows: *rows })
            }
            R::ShellInput { shell, data } => b64d(data).and_then(|d| self.shell_send(*shell, ShellOp::Data(d))),
            R::ShellResize { shell, cols, rows } => self.shell_send(*shell, ShellOp::Resize(*cols, *rows)),
            _ => return None,
        };
        Some(r.map(|_| Value::Null))
    }

    // --------------------------------------------------------- connection

    async fn ensure_connected(&self, host_id: &str) -> Result<Arc<SshConnection>, String> {
        let conn = self.conn(host_id);
        conn.wanted.store(true, Ordering::SeqCst);
        let _guard = conn.connecting.lock().await;
        let live = conn.ssh.lock().unwrap().clone().filter(|s| !s.is_closed());
        if let Some(s) = live {
            // Re-announce so late UI subscribers (reloaded page) see it.
            self.set_state(&conn, ConnState::Connected, None);
            return Ok(s);
        }
        self.set_state(&conn, ConnState::Connecting, None);
        match self.dial(host_id).await {
            Ok(ssh) => {
                *conn.ssh.lock().unwrap() = Some(ssh.clone());
                self.set_state(&conn, ConnState::Connected, None);
                self.spawn_monitor(conn.clone(), ssh.clone());
                self.start_pinned(host_id);
                Ok(ssh)
            }
            Err(e) => {
                self.set_state(&conn, ConnState::Error, Some(e.clone()));
                Err(e)
            }
        }
    }

    async fn dial(&self, host_id: &str) -> Result<Arc<SshConnection>, String> {
        let host = self.host(host_id)?;
        let target = self.inner.hosts.lock().unwrap().target(&host).map_err(|e| e.to_string())?;
        let prompter = Arc::new(UiPrompter {
            core: self.clone(),
            host_id: host_id.to_string(),
            checked: Mutex::new(HashSet::new()),
            used_saved: Mutex::new(HashSet::new()),
            pending: Mutex::new(Vec::new()),
            stale: Mutex::new(HashSet::new()),
        });
        let ssh = SshConnection::connect(&target, &self.inner.known_hosts, prompter.clone())
            .await
            .map(Arc::new)
            .map_err(|e| e.to_string())?;
        // Logged in: keep the secrets the user asked to save. (Never before:
        // a mistyped password must not end up in the vault.)
        prompter.commit();
        Ok(ssh)
    }

    fn spawn_monitor(&self, conn: Arc<Conn>, ssh: Arc<SshConnection>) {
        let c = self.clone();
        tokio::spawn(async move {
            while !ssh.is_closed() {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let still_current = conn.ssh.lock().unwrap().as_ref().is_some_and(|s| Arc::ptr_eq(s, &ssh));
            if !still_current || !conn.wanted.load(Ordering::SeqCst) {
                return;
            }
            c.reconnect_loop(conn).await;
        });
    }

    async fn reconnect_loop(&self, conn: Arc<Conn>) {
        let host_id = conn.host_id.clone();
        *conn.ssh.lock().unwrap() = None;
        // Plain shells die with the transport (that's what tmux is for).
        let dead: Vec<ShellId> = {
            let mut shells = self.inner.shells.lock().unwrap();
            let ids: Vec<ShellId> = shells.iter().filter(|(_, s)| s.host_id == host_id).map(|(k, _)| *k).collect();
            for i in &ids {
                shells.remove(i);
            }
            ids
        };
        for s in dead {
            self.emit(Event::ShellClosed { shell: s, reason: "connection lost".into() });
        }
        let mut delay = Duration::from_secs(1);
        let mut attempt = 0u32;
        loop {
            if !conn.wanted.load(Ordering::SeqCst) {
                return;
            }
            attempt += 1;
            self.set_state(&conn, ConnState::Reconnecting, Some(format!("attempt {attempt}")));
            let guard = conn.connecting.lock().await;
            let existing = conn.ssh.lock().unwrap().clone().filter(|s| !s.is_closed());
            let res = match existing {
                Some(s) => Ok(s), // someone else reconnected meanwhile
                None => self.dial(&host_id).await,
            };
            match res {
                Ok(ssh) => {
                    *conn.ssh.lock().unwrap() = Some(ssh.clone());
                    drop(guard);
                    self.set_state(&conn, ConnState::Connected, None);
                    self.spawn_monitor(conn.clone(), ssh.clone());
                    let ids: Vec<AttachmentId> = self
                        .inner
                        .attachments
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|(_, a)| a.host_id == host_id)
                        .map(|(k, _)| *k)
                        .collect();
                    self.reattach(&host_id, &ssh, &ids).await;
                    let _ = self.discover(&host_id).await;
                    return;
                }
                Err(e) => {
                    drop(guard);
                    tracing::info!("reconnect {host_id} failed: {e}");
                    self.set_state(&conn, ConnState::Reconnecting, Some(e));
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    /// Re-attach the given attachments to their sessions on a fresh SSH
    /// connection; close the ones whose session no longer exists.
    async fn reattach(&self, host_id: &str, ssh: &Arc<SshConnection>, ids: &[AttachmentId]) {
        let disc = match self.discover(host_id).await {
            Ok(d) => d,
            Err(_) => return,
        };
        for id in ids {
            let (tx, sid) = {
                let atts = self.inner.attachments.lock().unwrap();
                let Some(a) = atts.get(id) else { continue };
                if a.live {
                    continue;
                }
                let sid = *a.session.lock().unwrap();
                (a.tx.clone(), sid)
            };
            let Some(sid) = sid else {
                let _ = tx.send(AttachCmd::Close { reason: "session never attached".into() });
                continue;
            };
            if !disc.sessions.iter().any(|s| s.id == sid) {
                let _ = tx.send(AttachCmd::Close { reason: "the tmux session no longer exists".into() });
                continue;
            }
            match ssh.exec_stream(&remote::attach_command(sid)).await {
                Ok(stream) => {
                    let (client, events) = ControlClient::start(stream);
                    let _ = tx.send(AttachCmd::Rebind { client, events });
                }
                Err(e) => tracing::warn!("reattach {sid}: {e}"),
            }
        }
    }

    async fn disconnect(&self, host_id: &str) {
        let Some(conn) = self.inner.conns.lock().unwrap().get(host_id).cloned() else { return };
        conn.wanted.store(false, Ordering::SeqCst);
        let atts: Vec<mpsc::UnboundedSender<AttachCmd>> = self
            .inner
            .attachments
            .lock()
            .unwrap()
            .values()
            .filter(|a| a.host_id == host_id)
            .map(|a| a.tx.clone())
            .collect();
        for tx in atts {
            // Detach (never kill): tmux keeps the session and its processes.
            let _ = tx.send(AttachCmd::Detach);
        }
        let shells: Vec<ShellId> = {
            let mut sh = self.inner.shells.lock().unwrap();
            let ids: Vec<ShellId> = sh.iter().filter(|(_, s)| s.host_id == host_id).map(|(k, _)| *k).collect();
            for i in &ids {
                sh.remove(i);
            }
            ids
        };
        for s in shells {
            self.emit(Event::ShellClosed { shell: s, reason: "disconnected".into() });
        }
        // Pinned forwards stay pinned and come back on the next connect.
        let ids: Vec<TunnelId> =
            self.inner.tunnels.lock().unwrap().values().filter(|t| t.info.host_id == host_id).map(|t| t.info.id).collect();
        for id in ids {
            self.close_tunnel(id, false);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let ssh = conn.ssh.lock().unwrap().take();
        if let Some(ssh) = ssh {
            ssh.disconnect().await;
        }
        self.set_state(&conn, ConnState::Disconnected, None);
    }

    async fn discover(&self, host_id: &str) -> Result<remote::Discovery, String> {
        let ssh = self.current_ssh(host_id).filter(|s| !s.is_closed()).ok_or("not connected")?;
        let out = ssh.exec(&remote::discover_command()).await.map_err(|e| e.to_string())?;
        let d = remote::parse_discovery(&out.stdout_str());
        if let Some(e) = &d.error {
            self.emit(Event::Notice { level: "warn".into(), message: format!("tmux: {e}") });
        }
        self.emit(Event::Sessions { host_id: host_id.to_string(), tmux: d.tmux.clone(), sessions: d.sessions.clone() });
        Ok(d)
    }

    // --------------------------------------------------------- attachments

    async fn attach(&self, host_id: &str, target: AttachTarget, cols: u32, rows: u32) -> Result<AttachmentId, String> {
        self.attach_opts(host_id, target, cols, rows, false).await
    }

    async fn attach_opts(
        &self,
        host_id: &str,
        target: AttachTarget,
        cols: u32,
        rows: u32,
        popout: bool,
    ) -> Result<AttachmentId, String> {
        let ssh = self.ensure_connected(host_id).await?;
        let target = match target {
            AttachTarget::Auto => {
                let d = self.discover(host_id).await?;
                if d.tmux == TmuxStatus::NotInstalled {
                    return Err("tmux is not installed on this server".into());
                }
                // Never a pop-out's hidden companion session.
                match d.sessions.iter().filter(|s| !remote::is_popout_name(&s.name)).max_by_key(|s| s.activity) {
                    Some(s) => AttachTarget::Existing { session: s.id },
                    None => AttachTarget::New { name: "main".into() },
                }
            }
            t => t,
        };
        // Never open a second control client for a session we already show.
        if let AttachTarget::Existing { session } = &target {
            let existing = self.inner.attachments.lock().unwrap().iter().find_map(|(id, a)| {
                (a.host_id == host_id && *a.session.lock().unwrap() == Some(*session)).then(|| (*id, a.tx.clone(), a.popout))
            });
            if let Some((id, tx, popout)) = existing {
                let _ = tx.send(AttachCmd::Resize { cols, rows });
                let _ = tx.send(AttachCmd::Resync);
                self.emit(Event::Attached { attachment: id, host_id: host_id.to_string(), popout });
                return Ok(id);
            }
        }
        let command = match &target {
            AttachTarget::Existing { session } => remote::attach_command(*session),
            AttachTarget::New { name } => remote::new_session_command(name),
            AttachTarget::Auto => unreachable!(),
        };
        let stream = ssh.exec_stream(&command).await.map_err(|e| e.to_string())?;
        let (client, events) = ControlClient::start(stream);
        let id = self.id();
        let (tx, rx) = mpsc::unbounded_channel();
        let task = AttachmentTask::new(
            id,
            host_id.to_string(),
            client,
            events,
            rx,
            self.inner.internal.clone(),
            self.inner.sessions_changed.clone(),
            (cols, rows),
        );
        let slot = task.session_slot.clone();
        if let AttachTarget::Existing { session } = &target {
            *slot.lock().unwrap() = Some(*session);
        }
        self.inner
            .attachments
            .lock()
            .unwrap()
            .insert(id, Att { host_id: host_id.to_string(), popout, tx, session: slot, live: true });
        self.emit(Event::Attached { attachment: id, host_id: host_id.to_string(), popout });
        tokio::spawn(task.run());
        Ok(id)
    }

    /// Show `session` in another window: create a companion session in its
    /// group (shares the windows, has its own current window), attach to it
    /// and switch it to `window`.
    async fn pop_out(
        &self,
        host_id: &str,
        session: SessionId,
        window: Option<WindowId>,
        cols: u32,
        rows: u32,
    ) -> Result<(AttachmentId, SessionId), String> {
        let ssh = self.ensure_connected(host_id).await?;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let name = format!("{}{:x}{:x}", remote::POPOUT_MARK, nanos & 0xfffff, self.id());
        let out = ssh.exec(&remote::new_popout_session_command(session, &name)).await.map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(format!("could not open a new window: {}", out.stderr_str().trim()));
        }
        let sid: SessionId = out.stdout_str().trim().parse().map_err(|_| format!("unexpected tmux reply {:?}", out.stdout_str()))?;
        let att = self.attach_opts(host_id, AttachTarget::Existing { session: sid }, cols, rows, true).await?;
        if let Some(w) = window {
            let _ = self.att_command(att, cmd::select_window(sid, w)).await;
        }
        Ok((att, sid))
    }

    /// A pop-out window closed: detach and remove its companion session (its
    /// windows live on in the original session).
    async fn close_popout(&self, id: AttachmentId) -> Result<(), String> {
        let (host, sid) = {
            let atts = self.inner.attachments.lock().unwrap();
            let Some(a) = atts.get(&id) else { return Ok(()) };
            if !a.popout {
                return Err("not a pop-out window".into());
            }
            let sid = *a.session.lock().unwrap();
            (a.host_id.clone(), sid)
        };
        let _ = self.att_send(id, AttachCmd::Detach);
        if let (Some(sid), Some(ssh)) = (sid, self.current_ssh(&host).filter(|s| !s.is_closed())) {
            let _ = ssh.exec(&remote::kill_popout_session_command(sid)).await;
        }
        let _ = self.discover(&host).await;
        Ok(())
    }

    fn att_send(&self, id: AttachmentId, c: AttachCmd) -> Result<(), String> {
        let atts = self.inner.attachments.lock().unwrap();
        let a = atts.get(&id).ok_or("unknown attachment")?;
        a.tx.send(c).map_err(|_| "attachment closed".to_string())
    }

    fn att_session(&self, id: AttachmentId) -> Result<SessionId, String> {
        let atts = self.inner.attachments.lock().unwrap();
        let a = atts.get(&id).ok_or("unknown attachment")?;
        let s = *a.session.lock().unwrap();
        s.ok_or_else(|| "session not ready".to_string())
    }

    async fn att_command(&self, id: AttachmentId, line: String) -> Res {
        let (tx, rx) = oneshot::channel();
        self.att_send(id, AttachCmd::Command { line, reply: tx })?;
        rx.await.map_err(|_| "attachment closed".to_string())?.map(|_| Value::Null)
    }

    // -------------------------------------------------------------- shells

    async fn open_shell(&self, host_id: &str, cols: u32, rows: u32) -> Result<ShellId, String> {
        let ssh = self.ensure_connected(host_id).await?;
        let ch = ssh.open_shell(cols, rows).await.map_err(|e| e.to_string())?;
        let (mut read, write) = ch.split();
        let id = self.id();
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(op) = rx.recv().await {
                let r = match op {
                    ShellOp::Data(d) => write.data_bytes(d).await,
                    ShellOp::Resize(c, r) => write.window_change(c, r, 0, 0).await,
                    ShellOp::Close => {
                        let _ = write.eof().await;
                        write.close().await
                    }
                };
                if r.is_err() {
                    break;
                }
            }
        });
        self.inner.shells.lock().unwrap().insert(id, Shell { host_id: host_id.to_string(), tx });
        let c = self.clone();
        let host = host_id.to_string();
        tokio::spawn(async move {
            let mut reason = "shell exited".to_string();
            while let Some(msg) = read.wait().await {
                match msg {
                    mux_ssh::ChannelMsg::Data { data } | mux_ssh::ChannelMsg::ExtendedData { data, .. } => {
                        c.emit(Event::ShellOutput { shell: id, data: b64e(&data) });
                        c.scan_output(&host, format!("s{id}"), &data);
                    }
                    mux_ssh::ChannelMsg::ExitStatus { exit_status } => reason = format!("exited with status {exit_status}"),
                    mux_ssh::ChannelMsg::Close => break,
                    _ => {}
                }
            }
            c.inner.detectors.lock().unwrap().remove(&format!("s{id}"));
            if c.inner.shells.lock().unwrap().remove(&id).is_some() {
                c.emit(Event::ShellClosed { shell: id, reason });
            }
        });
        Ok(id)
    }

    fn shell_send(&self, id: ShellId, op: ShellOp) -> Result<(), String> {
        let shells = self.inner.shells.lock().unwrap();
        let s = shells.get(&id).ok_or("unknown shell")?;
        s.tx.send(op).map_err(|_| "shell closed".to_string())
    }

    // ------------------------------------------------------ port forwarding

    /// Look for "server started on port N" in terminal output and offer it.
    fn scan_output(&self, host_id: &str, key: String, data: &[u8]) {
        let found = self.inner.detectors.lock().unwrap().entry(key).or_default().feed(data);
        for d in found {
            let forwarded =
                self.inner.tunnels.lock().unwrap().values().any(|t| t.info.host_id == host_id && t.info.remote_port == d.port);
            if forwarded || !self.inner.suggested.lock().unwrap().insert((host_id.to_string(), d.port)) {
                continue;
            }
            // Only offer what is really listening: terminal text is full of
            // numbers that look like ports (and truncated ones).
            let c = self.clone();
            let host = host_id.to_string();
            tokio::spawn(async move {
                if c.is_listening(&host, d.port).await {
                    let forwarded =
                        c.inner.tunnels.lock().unwrap().values().any(|t| t.info.host_id == host && t.info.remote_port == d.port);
                    if !forwarded {
                        c.emit(Event::PortDetected { host_id: host, port: d.port, path: d.path });
                    }
                } else {
                    // Check again on a later mention, but not on every redraw.
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    c.inner.suggested.lock().unwrap().remove(&(host, d.port));
                }
            });
        }
    }

    /// Something on the server accepts connections on `localhost:port`
    /// (checked a few times: servers often print their URL just before
    /// they're ready). The test connection is closed right away.
    async fn is_listening(&self, host_id: &str, port: u16) -> bool {
        for delay in [0u64, 1000, 2000, 4000] {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let Some(ssh) = self.current_ssh(host_id).filter(|s| !s.is_closed()) else { return false };
            let origin = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
            if let Ok(Ok(ch)) = tokio::time::timeout(Duration::from_secs(3), ssh.open_direct_tcpip("localhost", port, origin)).await {
                drop(ch);
                return true;
            }
        }
        false
    }

    fn tunnel_list(&self) -> Vec<TunnelInfo> {
        let mut v: Vec<TunnelInfo> = self.inner.tunnels.lock().unwrap().values().map(|t| t.info.clone()).collect();
        v.sort_by(|a, b| (&a.host_id, a.remote_port).cmp(&(&b.host_id, b.remote_port)));
        v
    }

    fn emit_tunnels(&self) {
        self.emit(Event::Tunnels { tunnels: self.tunnel_list() });
    }

    fn saved_forward(info: &TunnelInfo) -> SavedForward {
        SavedForward {
            remote_port: info.remote_port,
            local_port: (info.local_port != info.remote_port).then_some(info.local_port),
            path: info.path.clone(),
        }
    }

    fn persist_pin(&self, info: &TunnelInfo) {
        let r = self.inner.hosts.lock().unwrap().set_pinned(&info.host_id, Self::saved_forward(info), info.pinned);
        if let Err(e) = r {
            self.emit(Event::Notice { level: "warn".into(), message: format!("could not save forward: {e}") });
        }
    }

    async fn open_tunnel(
        &self,
        host_id: &str,
        remote_port: u16,
        local_port: Option<u16>,
        path: Option<String>,
        pinned: bool,
    ) -> Result<TunnelInfo, String> {
        self.host(host_id)?;
        if remote_port == 0 {
            return Err("port must be between 1 and 65535".into());
        }
        let _guard = self.inner.tunnel_open.lock().await;
        // Already forwarded: reuse it (remembering a newly seen path / pin).
        let existing = {
            let mut ts = self.inner.tunnels.lock().unwrap();
            ts.values_mut().find(|t| t.info.host_id == host_id && t.info.remote_port == remote_port).map(|t| {
                let changed = (path.is_some() && t.info.path != path) || (pinned && !t.info.pinned);
                if path.is_some() {
                    t.info.path = path.clone();
                }
                t.info.pinned |= pinned;
                (t.info.clone(), changed)
            })
        };
        if let Some((info, changed)) = existing {
            if changed {
                if info.pinned {
                    self.persist_pin(&info);
                }
                self.emit_tunnels();
            }
            return Ok(info);
        }
        let listeners = bind_local(local_port.unwrap_or(remote_port), local_port.is_some()).await?;
        let bound = listeners[0].local_addr().map_err(|e| e.to_string())?.port();
        let id = self.id();
        let info = TunnelInfo {
            id,
            host_id: host_id.to_string(),
            remote_host: "localhost".into(),
            remote_port,
            local_port: bound,
            path: path.filter(|p| p.starts_with('/')),
            pinned,
            error: None,
        };
        let task = tokio::spawn(self.clone().serve_tunnel(id, host_id.to_string(), remote_port, listeners));
        self.inner.tunnels.lock().unwrap().insert(id, Tunnel { info: info.clone(), task });
        if pinned {
            self.persist_pin(&info);
        }
        tracing::info!("forwarding localhost:{bound} -> {host_id} localhost:{remote_port}");
        self.emit_tunnels();
        Ok(info)
    }

    /// `forget`: also unpin (the user stopped it), vs. a disconnect.
    fn close_tunnel(&self, id: TunnelId, forget: bool) {
        let Some(t) = self.inner.tunnels.lock().unwrap().remove(&id) else { return };
        t.task.abort();
        if forget {
            if t.info.pinned {
                let r = self.inner.hosts.lock().unwrap().set_pinned(&t.info.host_id, Self::saved_forward(&t.info), false);
                if let Err(e) = r {
                    tracing::warn!("unpin: {e}");
                }
            }
            // Let the port be offered again if the server prints it later.
            self.inner.suggested.lock().unwrap().remove(&(t.info.host_id.clone(), t.info.remote_port));
        }
        self.emit_tunnels();
    }

    fn pin_tunnel(&self, id: TunnelId, pinned: bool) -> Result<(), String> {
        let info = {
            let mut ts = self.inner.tunnels.lock().unwrap();
            let t = ts.get_mut(&id).ok_or("unknown tunnel")?;
            t.info.pinned = pinned;
            t.info.clone()
        };
        self.persist_pin(&info);
        self.emit_tunnels();
        Ok(())
    }

    /// Re-open the host's pinned forwards (after a fresh connect).
    fn start_pinned(&self, host_id: &str) {
        let saved = self.inner.hosts.lock().unwrap().pinned_forwards(host_id);
        if saved.is_empty() {
            return;
        }
        let c = self.clone();
        let host_id = host_id.to_string();
        tokio::spawn(async move {
            for f in saved {
                if let Err(e) = c.open_tunnel(&host_id, f.remote_port, f.local_port, f.path, true).await {
                    c.emit(Event::Notice {
                        level: "warn".into(),
                        message: format!("could not forward port {}: {e}", f.remote_port),
                    });
                }
            }
        });
    }

    async fn serve_tunnel(self, id: TunnelId, host_id: HostId, remote_port: u16, listeners: Vec<TcpListener>) {
        // Dropping this JoinSet (task aborted) aborts every connection too.
        let mut tasks = JoinSet::new();
        let (tx, mut rx) = mpsc::unbounded_channel::<(TcpStream, SocketAddr)>();
        for l in listeners {
            let tx = tx.clone();
            tasks.spawn(async move {
                loop {
                    match l.accept().await {
                        Ok(c) => {
                            if tx.send(c).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            // e.g. too many open files: back off, keep serving.
                            tracing::warn!("tunnel accept: {e}");
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    }
                }
            });
        }
        drop(tx);
        while let Some((tcp, peer)) = rx.recv().await {
            while tasks.try_join_next().is_some() {}
            let c = self.clone();
            let host = host_id.clone();
            tasks.spawn(async move { c.forward_one(id, &host, remote_port, tcp, peer).await });
        }
    }

    async fn forward_one(&self, id: TunnelId, host_id: &str, remote_port: u16, mut tcp: TcpStream, peer: SocketAddr) {
        let _ = tcp.set_nodelay(true);
        // During a reconnect, hold the browser's connection briefly instead
        // of failing it: the transport is usually back within seconds.
        let Some(ssh) = self.wait_for_ssh(host_id, Duration::from_secs(20)).await else {
            self.set_tunnel_error(id, Some("not connected to the server".into()));
            return;
        };
        match ssh.open_direct_tcpip("localhost", remote_port, peer).await {
            Ok(mut ch) => {
                self.set_tunnel_error(id, None);
                if let Err(e) = tokio::io::copy_bidirectional(&mut tcp, &mut ch).await {
                    tracing::debug!("tunnel {id}: {e}");
                }
            }
            Err(e) => {
                tracing::info!("tunnel {id} -> localhost:{remote_port}: {e}");
                self.set_tunnel_error(id, Some(format!("nothing is listening on port {remote_port} on the server")));
            }
        }
    }

    async fn wait_for_ssh(&self, host_id: &str, max: Duration) -> Option<Arc<SshConnection>> {
        let deadline = tokio::time::Instant::now() + max;
        loop {
            if let Some(s) = self.current_ssh(host_id).filter(|s| !s.is_closed()) {
                return Some(s);
            }
            let wanted = self.inner.conns.lock().unwrap().get(host_id).is_some_and(|c| c.wanted.load(Ordering::SeqCst));
            if !wanted || tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    fn set_tunnel_error(&self, id: TunnelId, error: Option<String>) {
        let changed = {
            let mut ts = self.inner.tunnels.lock().unwrap();
            match ts.get_mut(&id) {
                Some(t) if t.info.error != error => {
                    t.info.error = error;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.emit_tunnels();
        }
    }

    // --------------------------------------------------------------- vault

    fn emit_vault(&self) {
        let status = self.inner.vault.lock().unwrap().status();
        self.emit(Event::Vault { status });
    }

    /// Run a vault operation off the async threads (Argon2id takes a
    /// noticeable fraction of a second by design), then announce the state.
    async fn vault_blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Vault) -> Result<T, VaultError> + Send + 'static,
    ) -> Result<T, String> {
        let c = self.clone();
        let r = tokio::task::spawn_blocking(move || f(&mut c.inner.vault.lock().unwrap()))
            .await
            .map_err(|e| e.to_string())?;
        self.emit_vault();
        r.map_err(|e| e.to_string())
    }

    /// A saved secret for `slot`, unlocking the vault first if needed (asks
    /// for the master password; the user may decline and type the server
    /// password instead).
    async fn saved_secret(&self, host_id: &str, target: &str, slot: &str) -> Option<Zeroizing<String>> {
        {
            let v = self.inner.vault.lock().unwrap();
            if !v.has(slot) {
                return None;
            }
            if v.is_unlocked() {
                return v.get(slot).ok().flatten();
            }
        }
        for attempt in 0..3 {
            let req = PromptRequest::VaultUnlock { target: target.to_string(), retry: attempt > 0 };
            let Some(PromptAnswer::Secret { value: Some(master), save: remember }) = self.ask(host_id, req).await else {
                return None;
            };
            let master = Zeroizing::new(master.0);
            let r = self
                .vault_blocking(move |v| {
                    v.unlock(&master)?;
                    if remember {
                        v.set_remember(true)?;
                    }
                    Ok(())
                })
                .await;
            match r {
                Ok(()) => return self.inner.vault.lock().unwrap().get(slot).ok().flatten(),
                Err(e) if e == VaultError::WrongPassword.to_string() => continue,
                Err(e) => {
                    self.emit(Event::Notice { level: "warn".into(), message: format!("vault: {e}") });
                    return None;
                }
            }
        }
        None
    }

    // ------------------------------------------------------------- prompts

    async fn ask(&self, host_id: &str, request: PromptRequest) -> Option<PromptAnswer> {
        let id = self.id();
        let (tx, rx) = oneshot::channel();
        self.inner.prompts.lock().unwrap().insert(id, tx);
        self.emit(Event::Prompt { prompt: id, host_id: host_id.to_string(), request });
        let r = tokio::time::timeout(Duration::from_secs(600), rx).await.ok()?.ok();
        self.inner.prompts.lock().unwrap().remove(&id);
        r
    }
}

struct UiPrompter {
    core: Core,
    host_id: HostId,
    /// Slots whose saved secret was already looked up this connection.
    checked: Mutex<HashSet<String>>,
    /// Slots where the saved secret was offered to the server.
    used_saved: Mutex<HashSet<String>>,
    /// Typed secrets to save once the login succeeds.
    pending: Mutex<Vec<(String, Zeroizing<String>)>>,
    /// Slots whose saved secret the server rejected this connection.
    stale: Mutex<HashSet<String>>,
}

impl UiPrompter {
    /// The saved secret for `slot` the first time it's needed this
    /// connection; afterwards (it was rejected) the user is asked.
    async fn saved_once(&self, target: &str, slot: &str) -> Option<String> {
        if !self.checked.lock().unwrap().insert(slot.to_string()) {
            return None;
        }
        let s = self.core.saved_secret(&self.host_id, target, slot).await?;
        self.used_saved.lock().unwrap().insert(slot.to_string());
        Some(s.to_string())
    }

    fn after_answer(&self, slot: &str, answer: Option<PromptAnswer>) -> Option<String> {
        match answer {
            Some(PromptAnswer::Secret { value: Some(v), save }) => {
                // Only the latest answer for a slot can be the one that
                // worked: earlier (rejected) ones are dropped, so a typo
                // typed with "remember" on is never saved.
                let mut pending = self.pending.lock().unwrap();
                pending.retain(|(s, _)| s != slot);
                if save {
                    pending.push((slot.to_string(), Zeroizing::new(v.0.clone())));
                }
                Some(v.0)
            }
            _ => None,
        }
    }

    /// The login worked: store what the user asked to save (the last value
    /// typed for a slot is the one that was accepted).
    fn commit(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        // A saved secret the server rejected (and the user replaced without
        // re-saving) is known bad: drop it rather than retry it on every
        // reconnect (fail2ban / lockouts).
        let stale: Vec<String> = self
            .stale
            .lock()
            .unwrap()
            .iter()
            .filter(|s| !pending.iter().any(|(p, _)| p == *s))
            .cloned()
            .collect();
        if pending.is_empty() && stale.is_empty() {
            return;
        }
        let res: Result<(), VaultError> = {
            let mut v = self.core.inner.vault.lock().unwrap();
            stale
                .iter()
                .try_for_each(|slot| v.remove(slot).map(|_| ()))
                .and_then(|_| pending.iter().try_for_each(|(slot, secret)| v.put(slot, secret)))
        };
        if let Err(e) = res {
            self.core.emit(Event::Notice { level: "warn".into(), message: format!("Password not saved: {e}") });
        }
        self.core.emit_vault();
    }
}

#[async_trait::async_trait]
impl Prompter for UiPrompter {
    async fn unknown_host_key(&self, info: &HostKeyInfo) -> HostKeyDecision {
        match self.core.ask(&self.host_id, PromptRequest::HostKey { info: info.clone() }).await {
            Some(PromptAnswer::HostKey { accept: true, save }) => HostKeyDecision::from_accept(save),
            _ => HostKeyDecision::Reject,
        }
    }

    async fn password(&self, target: &str) -> Option<String> {
        let slot = vault::password_slot(target);
        if let Some(pw) = self.saved_once(target, &slot).await {
            return Some(pw);
        }
        let saved_failed = self.used_saved.lock().unwrap().contains(&slot);
        if saved_failed {
            self.stale.lock().unwrap().insert(slot.clone());
        }
        let req = PromptRequest::Password { target: target.into(), saved_failed };
        let answer = self.core.ask(&self.host_id, req).await;
        self.after_answer(&slot, answer)
    }

    async fn key_passphrase(&self, key_path: &Path) -> Option<String> {
        let slot = vault::passphrase_slot(key_path);
        let shown = key_path.display().to_string();
        if let Some(pw) = self.saved_once(&shown, &slot).await {
            return Some(pw);
        }
        let saved_failed = self.used_saved.lock().unwrap().contains(&slot);
        if saved_failed {
            self.stale.lock().unwrap().insert(slot.clone());
        }
        let req = PromptRequest::Passphrase { key_path: shown, saved_failed };
        let answer = self.core.ask(&self.host_id, req).await;
        self.after_answer(&slot, answer)
    }

    async fn keyboard_interactive(&self, target: &str, instructions: &str, prompts: &[(String, bool)]) -> Option<Vec<String>> {
        // Never saved: these are often one-time codes.
        let req = PromptRequest::KeyboardInteractive {
            target: target.into(),
            instructions: instructions.into(),
            prompts: prompts.to_vec(),
        };
        match self.core.ask(&self.host_id, req).await {
            Some(PromptAnswer::Responses { values }) => values.map(|v| v.into_iter().map(|s| s.0).collect()),
            _ => None,
        }
    }
}

/// Bind the local end of a tunnel on loopback only (never reachable from the
/// network). IPv4 is required; IPv6 `::1` on the same port is best effort, so
/// `localhost` works whichever address the browser tries first.
/// `strict`: exactly `preferred` or fail; otherwise the next free port.
async fn bind_local(preferred: u16, strict: bool) -> Result<Vec<TcpListener>, String> {
    let mut candidates: Vec<u16> = if strict {
        vec![preferred]
    } else {
        (0..20u16).filter_map(|i| preferred.checked_add(i)).collect()
    };
    if !strict {
        candidates.push(0); // anything free
    }
    let mut last = String::new();
    for p in candidates {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, p)).await {
            Ok(v4) => {
                let port = v4.local_addr().map_err(|e| e.to_string())?.port();
                let mut v = vec![v4];
                if let Ok(v6) = TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
                    v.push(v6);
                }
                return Ok(v);
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(if strict { format!("local port {preferred} is not available ({last})") } else { format!("no free local port ({last})") })
}
