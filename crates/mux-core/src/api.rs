//! The UI-facing contract. The Tauri shell and the browser dev bridge both
//! carry exactly these types as JSON, so the frontend is identical in both.

use crate::hosts::Host;
use mux_ssh::HostKeyInfo;
use serde::{Deserialize, Serialize};
use tmux_control::commands::SessionInfo;
use tmux_control::model::WindowState;
use tmux_control::{PaneId, SessionId, WindowId};

pub type HostId = String;
pub type AttachmentId = u64;
pub type ShellId = u64;
pub type PromptId = u64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request {
    // ---- hosts
    ListHosts,
    SaveHost { host: Host },
    DeleteHost { host_id: HostId },
    AddFolder { folder: String },
    // ---- connections
    Connect { host_id: HostId },
    Disconnect { host_id: HostId },
    RefreshSessions { host_id: HostId },
    // ---- tmux sessions
    Attach { host_id: HostId, target: AttachTarget, cols: u32, rows: u32 },
    Detach { attachment: AttachmentId },
    KillSession { host_id: HostId, session: SessionId },
    RenameSession { attachment: AttachmentId, name: String },
    Resize { attachment: AttachmentId, cols: u32, rows: u32 },
    // ---- windows (tabs)
    SelectWindow { attachment: AttachmentId, window: WindowId },
    NewWindow { attachment: AttachmentId },
    KillWindow { attachment: AttachmentId, window: WindowId },
    RenameWindow { attachment: AttachmentId, window: WindowId, name: String },
    // ---- panes
    SelectPane { attachment: AttachmentId, pane: PaneId },
    SplitPane { attachment: AttachmentId, pane: PaneId, horizontal: bool },
    KillPane { attachment: AttachmentId, pane: PaneId },
    ZoomPane { attachment: AttachmentId, pane: PaneId },
    MoveDivider { attachment: AttachmentId, window: WindowId, path: Vec<usize>, index: usize, delta: i32 },
    /// Base64 bytes typed/pasted into a pane.
    Input { attachment: AttachmentId, pane: PaneId, data: String },
    /// Request a fresh snapshot (e.g. the UI lost a terminal).
    RefreshPane { attachment: AttachmentId, pane: PaneId },
    // ---- plain shells
    OpenShell { host_id: HostId, cols: u32, rows: u32 },
    ShellInput { shell: ShellId, data: String },
    ShellResize { shell: ShellId, cols: u32, rows: u32 },
    CloseShell { shell: ShellId },
    // ---- prompts
    AnswerPrompt { prompt: PromptId, answer: PromptAnswer },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AttachTarget {
    /// Most recently used existing session, or a new "main" if none.
    Auto,
    Existing { session: SessionId },
    New { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PromptAnswer {
    HostKey { accept: bool, save: bool },
    Secret { value: Option<String> },
    Responses { values: Option<Vec<String>> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    /// Transport dropped; retrying. tmux sessions keep running remotely.
    Reconnecting,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TmuxStatus {
    Unknown,
    Available { version: String },
    NotInstalled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: Option<SessionId>,
    pub name: String,
    /// Ordered by index; layouts in cells.
    pub windows: Vec<WindowState>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Event {
    Hosts { hosts: Vec<Host>, folders: Vec<String> },
    Connection { host_id: HostId, state: ConnState, message: Option<String> },
    Sessions { host_id: HostId, tmux: TmuxStatus, sessions: Vec<SessionInfo> },
    Attached { attachment: AttachmentId, host_id: HostId },
    /// Whole session state; sent on every structural change (small).
    SessionState { attachment: AttachmentId, host_id: HostId, state: SessionView },
    /// Reset the pane's terminal to `cols`x`rows` and write `data`.
    PaneSnapshot { attachment: AttachmentId, pane: PaneId, cols: u32, rows: u32, data: String },
    PaneOutput { attachment: AttachmentId, pane: PaneId, data: String },
    /// Attachment is waiting for the connection to come back.
    AttachmentStatus { attachment: AttachmentId, live: bool },
    AttachmentClosed { attachment: AttachmentId, reason: String },
    ShellOutput { shell: ShellId, data: String },
    ShellClosed { shell: ShellId, reason: String },
    Prompt { prompt: PromptId, host_id: HostId, request: PromptRequest },
    Notice { level: String, message: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PromptRequest {
    HostKey { info: HostKeyInfo },
    Password { target: String },
    Passphrase { key_path: String },
    KeyboardInteractive { target: String, instructions: String, prompts: Vec<(String, bool)> },
}
