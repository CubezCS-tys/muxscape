// Mirror of crates/mux-core/src/api.rs. Keep in sync.

export type HostId = string;
export type AttachmentId = number;
export type ShellId = number;

export interface Host {
  id: HostId;
  label: string;
  folder: string;
  hostname: string;
  port: number;
  user?: string | null;
  identityFiles: string[];
  identitiesOnly: boolean;
  jump?: string | null;
  autoTmux: boolean;
  source: { kind: "saved" } | { kind: "sshConfig"; alias: string };
  /** Tile colour (#rrggbb); absent = derived from the label. */
  color?: string | null;
}

export type ConnState = "disconnected" | "connecting" | "connected" | "reconnecting" | "error";

export type TmuxStatus =
  | { kind: "unknown" }
  | { kind: "available"; version: string }
  | { kind: "notInstalled" };

export interface SessionInfo {
  id: string;
  name: string;
  windows: number;
  attached: number;
  created: number;
  activity: number;
}

export interface LayoutCell {
  width: number;
  height: number;
  x: number;
  y: number;
  type: "pane" | "horizontal" | "vertical";
  /** pane id for leaves, children for containers */
  value: string | LayoutCell[];
}

export interface WindowState {
  id: string;
  index: number;
  name: string;
  active: boolean;
  zoomed: boolean;
  bell: boolean;
  activity: boolean;
  layout: LayoutCell;
  visible_layout: LayoutCell;
  active_pane: string | null;
}

export interface SessionView {
  id: string | null;
  name: string;
  windows: WindowState[];
}

export interface HostKeyInfo {
  host: string;
  port: number;
  algorithm: string;
  fingerprint: string;
}

export type PromptRequest =
  | { kind: "hostKey"; info: HostKeyInfo }
  | { kind: "password"; target: string; savedFailed: boolean }
  | { kind: "passphrase"; keyPath: string; savedFailed: boolean }
  | { kind: "vaultUnlock"; target: string; retry: boolean }
  | { kind: "keyboardInteractive"; target: string; instructions: string; prompts: [string, boolean][] };

export type PromptAnswer =
  | { kind: "hostKey"; accept: boolean; save: boolean }
  /** save: keep in the vault (password/passphrase) or remember on this device (vaultUnlock). */
  | { kind: "secret"; value: string | null; save?: boolean }
  | { kind: "responses"; values: string[] | null };

/** Password vault state (never contains secrets). */
export interface VaultStatus {
  exists: boolean;
  unlocked: boolean;
  remember: boolean;
  rememberSupported: boolean;
  /** Saved slots: "password:user@host:port", "passphrase:/path/to/key". */
  entries: string[];
  error: string | null;
}

export type TunnelId = number;

export interface TunnelInfo {
  id: TunnelId;
  hostId: HostId;
  remoteHost: string;
  remotePort: number;
  localPort: number;
  path: string | null;
  pinned: boolean;
  /** Last forwarding problem, e.g. nothing listening on the server. */
  error: string | null;
}

export interface PaneRef {
  sessionId: string;
  sessionName: string;
  windowId: string;
  windowIndex: number;
  windowName: string;
  paneId: string;
}

/** A listening TCP port on the server (see crates/mux-core/src/ports.rs). */
export interface ListeningPort {
  port: number;
  address: string;
  /** Bound to all interfaces: reachable from other machines. */
  public: boolean;
  pid: number | null;
  /** Short name: vite, uvicorn, npm run dev, postgres… */
  process: string | null;
  command: string | null;
  /** Working directory, ~-relative. */
  cwd: string | null;
  pane: PaneRef | null;
  container: string | null;
  /** Started by the user (own process / tmux / container). */
  mine: boolean;
}

/** What a port answered. */
export interface PortProbe {
  port: number;
  web: boolean;
  tls: boolean;
  status: number | null;
  title: string | null;
  label: string | null;
  server: string | null;
  redirect: string | null;
  error: string | null;
}

export type AttachTarget =
  | { kind: "auto" }
  | { kind: "existing"; session: string }
  | { kind: "new"; name: string };

export type MuxEvent =
  | { event: "hosts"; hosts: Host[]; folders: string[] }
  | { event: "connection"; hostId: HostId; state: ConnState; message: string | null }
  | { event: "sessions"; hostId: HostId; tmux: TmuxStatus; sessions: SessionInfo[] }
  | { event: "attached"; attachment: AttachmentId; hostId: HostId; popout: boolean }
  | { event: "sessionState"; attachment: AttachmentId; hostId: HostId; state: SessionView }
  | { event: "paneSnapshot"; attachment: AttachmentId; pane: string; cols: number; rows: number; data: string }
  | { event: "paneOutput"; attachment: AttachmentId; pane: string; data: string }
  | { event: "attachmentStatus"; attachment: AttachmentId; live: boolean }
  | { event: "attachmentClosed"; attachment: AttachmentId; reason: string }
  | { event: "shellOutput"; shell: ShellId; data: string }
  | { event: "shellClosed"; shell: ShellId; reason: string }
  | { event: "prompt"; prompt: number; hostId: HostId; request: PromptRequest }
  | { event: "notice"; level: string; message: string }
  | { event: "vault"; status: VaultStatus }
  | { event: "tunnels"; tunnels: TunnelInfo[] }
  | { event: "portDetected"; hostId: HostId; port: number; path: string | null };

export type Request =
  | { cmd: "listHosts" }
  | { cmd: "saveHost"; host: Host }
  | { cmd: "deleteHost"; hostId: HostId }
  | { cmd: "addFolder"; folder: string }
  | { cmd: "connect"; hostId: HostId }
  | { cmd: "disconnect"; hostId: HostId }
  | { cmd: "refreshSessions"; hostId: HostId }
  | { cmd: "attach"; hostId: HostId; target: AttachTarget; cols: number; rows: number }
  | { cmd: "detach"; attachment: AttachmentId }
  | { cmd: "popOut"; hostId: HostId; session: string; window: string | null; cols: number; rows: number }
  | { cmd: "closePopout"; attachment: AttachmentId }
  | { cmd: "killSession"; hostId: HostId; session: string }
  | { cmd: "renameSession"; attachment: AttachmentId; name: string }
  | { cmd: "resize"; attachment: AttachmentId; cols: number; rows: number }
  | { cmd: "selectWindow"; attachment: AttachmentId; window: string }
  | { cmd: "newWindow"; attachment: AttachmentId }
  | { cmd: "killWindow"; attachment: AttachmentId; window: string }
  | { cmd: "renameWindow"; attachment: AttachmentId; window: string; name: string }
  | { cmd: "selectPane"; attachment: AttachmentId; pane: string }
  | { cmd: "splitPane"; attachment: AttachmentId; pane: string; horizontal: boolean }
  | { cmd: "killPane"; attachment: AttachmentId; pane: string }
  | { cmd: "zoomPane"; attachment: AttachmentId; pane: string }
  | { cmd: "moveDivider"; attachment: AttachmentId; window: string; path: number[]; index: number; delta: number }
  | { cmd: "input"; attachment: AttachmentId; pane: string; data: string }
  | { cmd: "refreshPane"; attachment: AttachmentId; pane: string }
  | { cmd: "openShell"; hostId: HostId; cols: number; rows: number }
  | { cmd: "shellInput"; shell: ShellId; data: string }
  | { cmd: "shellResize"; shell: ShellId; cols: number; rows: number }
  | { cmd: "closeShell"; shell: ShellId }
  | { cmd: "listTunnels" }
  | { cmd: "openTunnel"; hostId: HostId; remotePort: number; localPort?: number | null; path?: string | null; pinned?: boolean }
  | { cmd: "closeTunnel"; tunnel: TunnelId }
  | { cmd: "pinTunnel"; tunnel: TunnelId; pinned: boolean }
  | { cmd: "scanPorts"; hostId: HostId }
  | { cmd: "upload"; hostId: HostId; name: string; data: string }
  | { cmd: "probePorts"; hostId: HostId; targets: { port: number; process: string | null }[] }
  | { cmd: "vaultStatus" }
  | { cmd: "vaultCreate"; password: string; remember: boolean }
  | { cmd: "vaultUnlock"; password: string; remember: boolean }
  | { cmd: "vaultLock" }
  | { cmd: "vaultChangePassword"; old: string; new: string }
  | { cmd: "vaultSetRemember"; remember: boolean }
  | { cmd: "vaultForget"; slot: string }
  | { cmd: "vaultReset" }
  | { cmd: "answerPrompt"; prompt: number; answer: PromptAnswer };
