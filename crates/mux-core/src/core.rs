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
use crate::remote;
use base64::Engine;
use mux_ssh::{HostKeyDecision, HostKeyInfo, KnownHosts, Prompter, SshConnection};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tmux_control::commands as cmd;
use tmux_control::SessionId;
use tokio::sync::{mpsc, oneshot};

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
    tx: mpsc::UnboundedSender<AttachCmd>,
    session: Arc<Mutex<Option<SessionId>>>,
    live: bool,
}

struct Shell {
    host_id: HostId,
    write: Arc<mux_ssh::ChannelWriteHalf<mux_ssh::Msg>>,
}

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| e.to_string())
}

fn b64e(d: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(d)
}

impl Core {
    pub fn new(hosts: HostStore, known_hosts: KnownHosts) -> (Core, mpsc::UnboundedReceiver<Event>) {
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
            }),
        };
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

    pub async fn handle(&self, req: Request) -> Res {
        use Request as R;
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
            R::Resize { attachment, cols, rows } => {
                self.att_send(attachment, AttachCmd::Resize { cols, rows })?;
                Ok(Value::Null)
            }
            R::SelectWindow { attachment, window } => self.att_command(attachment, cmd::select_window(window)).await,
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
            R::Input { attachment, pane, data } => {
                self.att_send(attachment, AttachCmd::Input { pane, data: b64d(&data)? })?;
                Ok(Value::Null)
            }
            R::RefreshPane { attachment, pane } => {
                self.att_send(attachment, AttachCmd::RefreshPane { pane })?;
                Ok(Value::Null)
            }
            R::OpenShell { host_id, cols, rows } => {
                let id = self.open_shell(&host_id, cols, rows).await?;
                Ok(json!({ "shell": id }))
            }
            R::ShellInput { shell, data } => {
                let w = self.shell_writer(shell)?;
                w.data_bytes(b64d(&data)?).await.map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            R::ShellResize { shell, cols, rows } => {
                let w = self.shell_writer(shell)?;
                w.window_change(cols, rows, 0, 0).await.map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            R::CloseShell { shell } => {
                if let Some(s) = self.inner.shells.lock().unwrap().remove(&shell) {
                    let w = s.write.clone();
                    tokio::spawn(async move {
                        let _ = w.eof().await;
                        let _ = w.close().await;
                    });
                }
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

    // --------------------------------------------------------- connection

    async fn ensure_connected(&self, host_id: &str) -> Result<Arc<SshConnection>, String> {
        let conn = self.conn(host_id);
        conn.wanted.store(true, Ordering::SeqCst);
        let _guard = conn.connecting.lock().await;
        if let Some(s) = conn.ssh.lock().unwrap().clone() {
            if !s.is_closed() {
                return Ok(s);
            }
        }
        self.set_state(&conn, ConnState::Connecting, None);
        match self.dial(host_id).await {
            Ok(ssh) => {
                *conn.ssh.lock().unwrap() = Some(ssh.clone());
                self.set_state(&conn, ConnState::Connected, None);
                self.spawn_monitor(conn.clone(), ssh.clone());
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
        let prompter = Arc::new(UiPrompter { core: self.clone(), host_id: host_id.to_string() });
        SshConnection::connect(&target, &self.inner.known_hosts, prompter)
            .await
            .map(Arc::new)
            .map_err(|e| e.to_string())
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
        let ssh = self.ensure_connected(host_id).await?;
        let target = match target {
            AttachTarget::Auto => {
                let d = self.discover(host_id).await?;
                if d.tmux == TmuxStatus::NotInstalled {
                    return Err("tmux is not installed on this server".into());
                }
                match d.sessions.iter().max_by_key(|s| s.activity) {
                    Some(s) => AttachTarget::Existing { session: s.id },
                    None => AttachTarget::New { name: "main".into() },
                }
            }
            t => t,
        };
        // Never open a second control client for a session we already show.
        if let AttachTarget::Existing { session } = &target {
            let existing = self.inner.attachments.lock().unwrap().iter().find_map(|(id, a)| {
                (a.host_id == host_id && *a.session.lock().unwrap() == Some(*session)).then(|| (*id, a.tx.clone()))
            });
            if let Some((id, tx)) = existing {
                let _ = tx.send(AttachCmd::Resize { cols, rows });
                let _ = tx.send(AttachCmd::Resync);
                self.emit(Event::Attached { attachment: id, host_id: host_id.to_string() });
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
            .insert(id, Att { host_id: host_id.to_string(), tx, session: slot, live: true });
        self.emit(Event::Attached { attachment: id, host_id: host_id.to_string() });
        tokio::spawn(task.run());
        Ok(id)
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
        self.inner
            .shells
            .lock()
            .unwrap()
            .insert(id, Shell { host_id: host_id.to_string(), write: Arc::new(write) });
        let c = self.clone();
        tokio::spawn(async move {
            let mut reason = "shell exited".to_string();
            while let Some(msg) = read.wait().await {
                match msg {
                    mux_ssh::ChannelMsg::Data { data } | mux_ssh::ChannelMsg::ExtendedData { data, .. } => {
                        c.emit(Event::ShellOutput { shell: id, data: b64e(&data) });
                    }
                    mux_ssh::ChannelMsg::ExitStatus { exit_status } => reason = format!("exited with status {exit_status}"),
                    mux_ssh::ChannelMsg::Close => break,
                    _ => {}
                }
            }
            if c.inner.shells.lock().unwrap().remove(&id).is_some() {
                c.emit(Event::ShellClosed { shell: id, reason });
            }
        });
        Ok(id)
    }

    fn shell_writer(&self, id: ShellId) -> Result<Arc<mux_ssh::ChannelWriteHalf<mux_ssh::Msg>>, String> {
        self.inner.shells.lock().unwrap().get(&id).map(|s| s.write.clone()).ok_or_else(|| "unknown shell".into())
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
        match self.core.ask(&self.host_id, PromptRequest::Password { target: target.into() }).await {
            Some(PromptAnswer::Secret { value }) => value,
            _ => None,
        }
    }

    async fn key_passphrase(&self, key_path: &Path) -> Option<String> {
        let req = PromptRequest::Passphrase { key_path: key_path.display().to_string() };
        match self.core.ask(&self.host_id, req).await {
            Some(PromptAnswer::Secret { value }) => value,
            _ => None,
        }
    }

    async fn keyboard_interactive(&self, target: &str, instructions: &str, prompts: &[(String, bool)]) -> Option<Vec<String>> {
        let req = PromptRequest::KeyboardInteractive {
            target: target.into(),
            instructions: instructions.into(),
            prompts: prompts.to_vec(),
        };
        match self.core.ask(&self.host_id, req).await {
            Some(PromptAnswer::Responses { values }) => values,
            _ => None,
        }
    }
}
