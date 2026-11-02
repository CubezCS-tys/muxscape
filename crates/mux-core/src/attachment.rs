//! One attached tmux session: a control-mode client plus the state machine
//! that keeps the GUI in sync with it.
//!
//! The task processes control events strictly in stream order. Anything
//! that loads state (window/pane snapshots, screen captures) is requested
//! with tagged replies so it is applied at the exact point in the stream
//! where tmux produced it — never on top of newer notifications.
//!
//! If the transport drops, the task parks in "reconnecting" and keeps its
//! id; the connection manager hands it a fresh control client once SSH is
//! back ([`AttachCmd::Rebind`]) and the task rebuilds everything from tmux.

use crate::api::{AttachmentId, Event, HostId, SessionView};
use crate::control::{ControlClient, ControlEvent};
use base64::Engine;
use std::collections::{HashMap, HashSet};
use tmux_control::commands::{self as cmd, Capture, PaneInfo};
use tmux_control::model::Effect;
use tmux_control::snapshot::{self, Captures};
use tmux_control::{Notification, PaneId, Response, SessionId, SessionModel, WindowId};
use tokio::sync::mpsc;

/// Lines of scrollback restored per pane on attach/reconnect.
pub const RESTORE_SCROLLBACK: u32 = 2000;

pub enum AttachCmd {
    Resize { cols: u32, rows: u32 },
    Input { pane: PaneId, data: Vec<u8> },
    /// Arbitrary tmux command (already built and quoted).
    Command { line: String, reply: tokio::sync::oneshot::Sender<Result<(), String>> },
    MoveDivider { window: WindowId, path: Vec<usize>, index: usize, delta: i32, reply: tokio::sync::oneshot::Sender<Result<(), String>> },
    RefreshPane { pane: PaneId },
    /// Re-send full state and fresh snapshots (UI reloaded).
    Resync,
    /// End the attachment with a reason (session vanished...).
    Close { reason: String },
    Rebind { client: ControlClient, events: mpsc::UnboundedReceiver<ControlEvent> },
    Detach,
}

#[derive(Debug, Clone)]
enum Tag {
    Windows,
    Panes,
    PaneInfo(PaneId, u64),
    PaneHistory(PaneId, u64),
    PaneScreen(PaneId, u64),
    PaneSaved(PaneId, u64),
}

#[derive(Default)]
struct PendingSnapshot {
    info: Option<PaneInfo>,
    history: Vec<Vec<u8>>,
    screen: Option<Vec<Vec<u8>>>,
}

pub struct AttachmentTask {
    pub id: AttachmentId,
    pub host_id: HostId,
    client: Option<ControlClient>,
    events: Option<mpsc::UnboundedReceiver<ControlEvent>>,
    cmds: mpsc::UnboundedReceiver<AttachCmd>,
    out: mpsc::UnboundedSender<Event>,
    /// Ask the connection manager to refresh the server's session list.
    sessions_changed: mpsc::UnboundedSender<HostId>,
    model: SessionModel,
    size: (u32, u32),
    next_tag: u64,
    tags: HashMap<u64, Tag>,
    pending_windows: Option<Vec<tmux_control::commands::WindowInfo>>,
    resync_in_flight: bool,
    resync_again: bool,
    /// Panes whose output is suppressed until their snapshot lands, with the
    /// generation of the snapshot request we're waiting for.
    syncing: HashMap<PaneId, u64>,
    snapshots: HashMap<PaneId, PendingSnapshot>,
    /// Panes that have had a snapshot delivered to the UI.
    synced: HashSet<PaneId>,
    generation: u64,
    /// Shared with the connection manager: which session we're attached to.
    pub session_slot: std::sync::Arc<std::sync::Mutex<Option<SessionId>>>,
}

enum Flow {
    Continue,
    /// Transport lost; wait for Rebind.
    Lost,
    /// tmux ended this client (detached, session killed...).
    Ended(String),
}

impl AttachmentTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: AttachmentId,
        host_id: HostId,
        client: ControlClient,
        events: mpsc::UnboundedReceiver<ControlEvent>,
        cmds: mpsc::UnboundedReceiver<AttachCmd>,
        out: mpsc::UnboundedSender<Event>,
        sessions_changed: mpsc::UnboundedSender<HostId>,
        size: (u32, u32),
    ) -> Self {
        AttachmentTask {
            id,
            host_id,
            client: Some(client),
            events: Some(events),
            cmds,
            out,
            sessions_changed,
            model: SessionModel::new(),
            size,
            next_tag: 1,
            tags: HashMap::new(),
            pending_windows: None,
            resync_in_flight: false,
            resync_again: false,
            syncing: HashMap::new(),
            snapshots: HashMap::new(),
            synced: HashSet::new(),
            generation: 0,
            session_slot: Default::default(),
        }
    }

    pub fn session_id(&self) -> Option<SessionId> {
        self.model.session_id
    }

    fn emit(&self, e: Event) {
        let _ = self.out.send(e);
    }

    fn b64(data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    fn tag(&mut self, t: Tag) -> u64 {
        let n = self.next_tag;
        self.next_tag += 1;
        self.tags.insert(n, t);
        n
    }

    fn client(&self) -> Option<&ControlClient> {
        self.client.as_ref()
    }

    /// (Re)initialise after attach: size, then a full resync.
    fn start(&mut self) {
        self.model = SessionModel::new();
        self.tags.clear();
        self.pending_windows = None;
        self.resync_in_flight = false;
        self.syncing.clear();
        self.snapshots.clear();
        self.synced.clear();
        if let Some(c) = self.client() {
            c.send(&cmd::refresh_client_size(self.size.0, self.size.1));
        }
        // The session id arrives with %session-changed right after attach,
        // which triggers the first resync.
    }

    fn request_resync(&mut self) {
        let Some(sid) = self.model.session_id else { return };
        if self.resync_in_flight {
            self.resync_again = true;
            return;
        }
        let t1 = self.tag(Tag::Windows);
        let t2 = self.tag(Tag::Panes);
        let batch = vec![(cmd::list_windows(sid), t1), (cmd::list_panes_session(sid), t2)];
        if let Some(c) = self.client() {
            if c.call_tagged_batch(&batch).is_ok() {
                self.resync_in_flight = true;
            }
        }
    }

    fn request_snapshot(&mut self, pane: PaneId) {
        self.generation += 1;
        let g = self.generation;
        self.syncing.insert(pane, g);
        self.snapshots.insert(pane, PendingSnapshot::default());
        // One batch = one consistent instant (see ControlClient docs).
        let batch = vec![
            (cmd::describe_pane(pane), self.tag(Tag::PaneInfo(pane, g))),
            (cmd::capture_pane(pane, Capture::History(RESTORE_SCROLLBACK)), self.tag(Tag::PaneHistory(pane, g))),
            (cmd::capture_pane(pane, Capture::Screen), self.tag(Tag::PaneScreen(pane, g))),
            (cmd::capture_pane(pane, Capture::SavedNormal), self.tag(Tag::PaneSaved(pane, g))),
        ];
        if let Some(cl) = self.client() {
            let _ = cl.call_tagged_batch(&batch);
        }
    }

    fn emit_state(&self) {
        *self.session_slot.lock().unwrap() = self.model.session_id;
        self.emit(Event::SessionState {
            attachment: self.id,
            host_id: self.host_id.clone(),
            state: SessionView {
                id: self.model.session_id,
                name: self.model.session_name.clone(),
                windows: self.model.windows.clone(),
            },
        });
    }

    fn on_reply(&mut self, tag: u64, r: Response) {
        let Some(t) = self.tags.remove(&tag) else { return };
        match t {
            Tag::Windows => {
                if r.success {
                    let lines: Vec<String> = r.lines().collect();
                    match cmd::parse_windows(lines.iter().map(|s| s.as_str())) {
                        Ok(w) => self.pending_windows = Some(w),
                        Err(e) => tracing::warn!("list-windows: {e}"),
                    }
                }
            }
            Tag::Panes => {
                self.resync_in_flight = false;
                let windows = self.pending_windows.take();
                let lines: Vec<String> = r.lines().collect();
                let panes = cmd::parse_panes(lines.iter().map(|s| s.as_str()));
                match (windows, panes) {
                    (Some(w), Ok(p)) if r.success => {
                        self.model.load_snapshot(w, p);
                        self.emit_state();
                        // Snapshot panes we haven't drawn yet.
                        let live: HashSet<PaneId> = self.model.all_panes().into_iter().collect();
                        self.synced.retain(|p| live.contains(p));
                        let new: Vec<PaneId> = live
                            .iter()
                            .filter(|p| !self.synced.contains(p) && !self.syncing.contains_key(p))
                            .copied()
                            .collect();
                        for p in new {
                            self.request_snapshot(p);
                        }
                    }
                    (_, Err(e)) => tracing::warn!("list-panes: {e}"),
                    _ => tracing::warn!("resync failed: {}", r.text()),
                }
                if std::mem::take(&mut self.resync_again) {
                    self.request_resync();
                }
            }
            Tag::PaneInfo(p, g) => {
                if self.syncing.get(&p) == Some(&g) {
                    let lines: Vec<String> = r.lines().collect();
                    let info = cmd::parse_panes(lines.iter().map(|s| s.as_str())).ok().and_then(|mut v| v.pop());
                    if let Some(s) = self.snapshots.get_mut(&p) {
                        s.info = info;
                    }
                }
            }
            Tag::PaneHistory(p, g) => {
                if self.syncing.get(&p) == Some(&g) {
                    if let Some(s) = self.snapshots.get_mut(&p) {
                        // Without history tmux returns screen line 0 here.
                        if r.success && s.info.as_ref().is_some_and(|i| i.history_size > 0) {
                            s.history = r.output;
                        }
                    }
                }
            }
            Tag::PaneScreen(p, g) => {
                if self.syncing.get(&p) == Some(&g) {
                    if let Some(s) = self.snapshots.get_mut(&p) {
                        s.screen = r.success.then_some(r.output);
                    }
                }
            }
            Tag::PaneSaved(p, g) => {
                if self.syncing.get(&p) != Some(&g) {
                    return;
                }
                self.syncing.remove(&p);
                let Some(snap) = self.snapshots.remove(&p) else { return };
                let (Some(info), Some(screen)) = (snap.info, snap.screen) else {
                    // Pane vanished between request and reply.
                    return;
                };
                let saved = r.success.then_some(r.output);
                let data = snapshot::build(
                    &info,
                    &Captures {
                        history: &snap.history,
                        screen: &screen,
                        saved_normal: if info.alternate_on { saved.as_deref() } else { None },
                    },
                );
                self.synced.insert(p);
                if let Some(pi) = self.model.panes.get_mut(&p) {
                    *pi = info.clone();
                }
                self.emit(Event::PaneSnapshot {
                    attachment: self.id,
                    pane: p,
                    cols: info.width,
                    rows: info.height,
                    data: Self::b64(&data),
                });
            }
        }
    }

    fn on_notification(&mut self, n: Notification) -> Flow {
        match &n {
            Notification::Output { pane, data } | Notification::ExtendedOutput { pane, data, .. } => {
                if !self.syncing.contains_key(pane) {
                    self.emit(Event::PaneOutput { attachment: self.id, pane: *pane, data: Self::b64(data) });
                }
                return Flow::Continue;
            }
            Notification::Pause { pane } => {
                // Only happens with pause-after; resume and redraw.
                if let Some(c) = self.client() {
                    c.send(&format!("refresh-client -A {}", tmux_control::escape::quote(&format!("{pane}:continue"))));
                }
                self.request_snapshot(*pane);
                return Flow::Continue;
            }
            _ => {}
        }
        match self.model.apply(&n) {
            Effect::None => {}
            Effect::Changed => {
                self.emit_state();
                // Panes removed from layouts are forgotten.
                let live: HashSet<PaneId> = self.model.all_panes().into_iter().collect();
                self.synced.retain(|p| live.contains(p));
            }
            Effect::Resync => self.request_resync(),
            Effect::SessionSwitched => {
                self.synced.clear();
                self.syncing.clear();
                self.emit_state();
                self.request_resync();
                let _ = self.sessions_changed.send(self.host_id.clone());
            }
            Effect::SessionListChanged => {
                self.emit_state();
                let _ = self.sessions_changed.send(self.host_id.clone());
            }
            Effect::Exit(reason) => {
                return Flow::Ended(reason.unwrap_or_else(|| "tmux client exited".into()));
            }
        }
        Flow::Continue
    }

    fn on_event(&mut self, ev: ControlEvent) -> Flow {
        match ev {
            ControlEvent::Notification(n) => self.on_notification(n),
            ControlEvent::Reply { tag, response } => {
                self.on_reply(tag, response);
                Flow::Continue
            }
            ControlEvent::Stray(s) => {
                tracing::debug!("stray: {s}");
                if self.model.session_id.is_none() && !s.trim().is_empty() {
                    // e.g. "can't find session: $4" or "sessions should be nested with care"
                    self.emit(Event::Notice { level: "warn".into(), message: s });
                }
                Flow::Continue
            }
            ControlEvent::Closed { exit: Some(reason) } => Flow::Ended(reason.unwrap_or_else(|| "detached".into())),
            ControlEvent::Closed { exit: None } => Flow::Lost,
        }
    }

    async fn on_cmd(&mut self, c: AttachCmd) -> Option<Flow> {
        match c {
            AttachCmd::Resize { cols, rows } => {
                self.size = (cols, rows);
                if let Some(cl) = self.client() {
                    cl.send(&cmd::refresh_client_size(cols, rows));
                }
            }
            AttachCmd::Input { pane, data } => {
                if let Some(cl) = self.client() {
                    for line in cmd::send_bytes(pane, &data) {
                        cl.send(&line);
                    }
                }
            }
            AttachCmd::Command { line, reply } => match self.client().cloned() {
                Some(cl) => {
                    tokio::spawn(async move {
                        let r = cl.call(&line).await.map(|_| ()).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    });
                }
                None => {
                    let _ = reply.send(Err("not connected".into()));
                }
            },
            AttachCmd::MoveDivider { window, path, index, delta, reply } => {
                let r = self.move_divider(window, &path, index, delta).await;
                let _ = reply.send(r);
            }
            AttachCmd::RefreshPane { pane } => self.request_snapshot(pane),
            AttachCmd::Resync => {
                self.synced.clear();
                self.syncing.clear();
                self.emit_state();
                self.request_resync();
            }
            AttachCmd::Close { reason } => {
                if let Some(c) = self.client() {
                    c.detach();
                }
                return Some(Flow::Ended(reason));
            }
            AttachCmd::Rebind { client, events } => {
                self.client = Some(client);
                self.events = Some(events);
                self.start();
                self.emit(Event::AttachmentStatus { attachment: self.id, live: true });
            }
            AttachCmd::Detach => {
                if let Some(c) = self.client() {
                    c.detach();
                }
                return Some(Flow::Ended("detached".into()));
            }
        }
        None
    }

    async fn move_divider(&mut self, window: WindowId, path: &[usize], index: usize, delta: i32) -> Result<(), String> {
        let w = self.model.window(window).ok_or("unknown window")?;
        let (layout, applied) = w.layout.move_divider(path, index, delta).ok_or("invalid divider")?;
        if applied == 0 {
            return Ok(());
        }
        let cl = self.client().cloned().ok_or("not connected")?;
        // select-layout assigns panes to cells in the window's pane-list
        // order; only safe when that matches layout order (it virtually
        // always does). Otherwise fall back to relative resize-pane.
        let order = cl.call(&cmd::list_window_pane_order(window)).await.map_err(|e| e.to_string())?;
        let order: Vec<String> = order.lines().collect();
        let leaves: Vec<String> = w.layout.panes().iter().map(|p| p.to_string()).collect();
        if order == leaves {
            cl.call(&cmd::select_layout(window, &layout)).await.map_err(|e| e.to_string())?;
        } else {
            let node = node_at(&w.layout, path).ok_or("invalid divider")?;
            let child = &node.children()[index];
            let pane = child.panes().last().copied().ok_or("empty cell")?;
            let horizontal = matches!(node.kind, tmux_control::CellKind::Horizontal(_));
            let (flag, n) = match (horizontal, applied > 0) {
                (true, true) => ("-R", applied),
                (true, false) => ("-L", -applied),
                (false, true) => ("-D", applied),
                (false, false) => ("-U", -applied),
            };
            cl.call(&format!("resize-pane -t {} {} {}", cmd::t(pane), flag, n)).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn run(mut self) {
        self.start();
        loop {
            let flow = match self.events.as_mut() {
                Some(events) => {
                    tokio::select! {
                        ev = events.recv() => match ev {
                            Some(ev) => self.on_event(ev),
                            None => Flow::Lost,
                        },
                        c = self.cmds.recv() => match c {
                            Some(c) => self.on_cmd(c).await.unwrap_or(Flow::Continue),
                            None => Flow::Ended("closed".into()),
                        },
                    }
                }
                None => match self.cmds.recv().await {
                    Some(c) => self.on_cmd(c).await.unwrap_or(Flow::Continue),
                    None => Flow::Ended("closed".into()),
                },
            };
            match flow {
                Flow::Continue => {}
                Flow::Lost => {
                    if self.events.is_some() {
                        self.events = None;
                        self.client = None;
                        self.emit(Event::AttachmentStatus { attachment: self.id, live: false });
                    }
                }
                Flow::Ended(reason) => {
                    self.emit(Event::AttachmentClosed { attachment: self.id, reason });
                    let _ = self.sessions_changed.send(self.host_id.clone());
                    return;
                }
            }
        }
    }
}

fn node_at<'a>(l: &'a tmux_control::LayoutCell, path: &[usize]) -> Option<&'a tmux_control::LayoutCell> {
    let mut cur = l;
    for &i in path {
        cur = cur.children().get(i)?;
    }
    Some(cur)
}
