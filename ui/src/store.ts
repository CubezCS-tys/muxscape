import { create } from "zustand";
import { backend, b64ToBytes, closeCurrentWindow } from "./backend";
import { applyPrefs, loadPrefs, savePrefs, type Prefs } from "./prefs";
import { disposeWhere, getTerminal, paneKey, peekTerminal, shellKey } from "./terminals";
import type {
  AttachmentId, ConnState, Host, HostId, ListeningPort, MuxEvent, PortProbe, PromptRequest, SessionInfo, SessionView, ShellId,
  TmuxStatus, TunnelInfo, VaultStatus,
} from "./types";

export type View = { kind: "att"; id: AttachmentId } | { kind: "shell"; id: ShellId };

/** Set when this window is a pop-out (one session, opened from a tab). */
export const POPOUT: { attachment: AttachmentId; hostId: HostId; session: string; name: string; window: string } | null = (() => {
  try {
    const q = new URLSearchParams(location.search);
    const a = Number(q.get("popout"));
    const hostId = q.get("host");
    const session = q.get("session");
    const window = q.get("window");
    return a && hostId && session && window ? { attachment: a, hostId, session, name: q.get("name") ?? "", window } : null;
  } catch {
    return null;
  }
})();

export interface Attachment {
  id: AttachmentId;
  hostId: HostId;
  state: SessionView | null;
  live: boolean;
}

export interface ShellView {
  id: ShellId;
  hostId: HostId;
  closed: string | null;
}

export interface Prompt {
  prompt: number;
  hostId: HostId;
  request: PromptRequest;
}

export interface Toast {
  id: number;
  level: "info" | "warn" | "error";
  message: string;
}

/** "A server just started on port N" offer, shown as a chip. */
export interface DetectedPort {
  hostId: HostId;
  port: number;
  path: string | null;
}

/** Last scan of a host's listening ports, plus probe results. */
export interface PortInfo {
  ports: ListeningPort[];
  /** Keyed by `${port}:${pid}`: re-probed when the process changes. */
  probes: Record<string, PortProbe>;
  scannedAt: number;
  scanning: boolean;
}

export const probeKey = (port: number, pid: number | null | undefined) => `${port}:${pid ?? ""}`;

export interface ServerInfo {
  tmux: TmuxStatus;
  sessions: SessionInfo[];
}

interface State {
  hosts: Host[];
  folders: string[];
  conn: Record<HostId, { state: ConnState; message: string | null }>;
  servers: Record<HostId, ServerInfo>;
  attachments: Record<AttachmentId, Attachment>;
  shells: Record<ShellId, ShellView>;
  views: View[];
  active: View | null;
  prompts: Prompt[];
  toasts: Toast[];
  /** Workspace viewport in cells (what we tell tmux). */
  viewport: { cols: number; rows: number };
  expanded: Record<HostId, boolean>;
  switcherOpen: boolean;
  hostDialog: Host | "new" | null;
  tunnels: TunnelInfo[];
  detected: DetectedPort[];
  /** Ports side panel. */
  portsOpen: boolean;
  /** Host shown in the panel (null: follow the active view's host). */
  portsHost: HostId | null;
  portInfo: Record<HostId, PortInfo>;
  vault: VaultStatus | null;
  vaultDialog: boolean;
  prefs: Prefs;
  appearanceOpen: boolean;
  /** Tabs this window popped out: pop-out attachment → (our attachment, tmux window). */
  popped: Record<AttachmentId, { from: AttachmentId; window: string }>;
}

export const useStore = create<State>(() => ({
  hosts: [],
  folders: [],
  conn: {},
  servers: {},
  attachments: {},
  shells: {},
  views: [],
  active: null,
  prompts: [],
  toasts: [],
  viewport: { cols: 120, rows: 36 },
  expanded: {},
  switcherOpen: false,
  hostDialog: null,
  tunnels: [],
  detected: [],
  portsOpen: false,
  portsHost: null,
  portInfo: {},
  vault: null,
  vaultDialog: false,
  prefs: loadPrefs(),
  appearanceOpen: false,
  popped: {},
}));

/**
 * The tabs a window shows for an attachment: a pop-out only its one tab; the
 * main window everything except the tabs it popped out (a tab "moves" into
 * its own window, like a browser tab).
 */
export function visibleWindows<W extends { id: string }>(att: AttachmentId, windows: W[], popped = useStore.getState().popped): W[] {
  if (POPOUT) return windows.filter((w) => w.id === POPOUT!.window);
  const out = new Set(Object.values(popped).filter((p) => p.from === att).map((p) => p.window));
  return windows.filter((w) => !out.has(w.id));
}

/** Change appearance: applied immediately (live preview) and remembered. */
export function setPrefs(patch: Partial<Prefs>) {
  const prefs = { ...useStore.getState().prefs, ...patch };
  useStore.setState({ prefs });
  applyPrefs(prefs);
  savePrefs(prefs);
}

const set = useStore.setState;
const get = useStore.getState;

let toastId = 1;
export function toast(message: string, level: Toast["level"] = "info") {
  const id = toastId++;
  set((s) => ({ toasts: [...s.toasts, { id, level, message }] }));
  setTimeout(() => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })), level === "error" ? 8000 : 4000);
}

export function sameView(a: View | null, b: View | null) {
  return !!a && !!b && a.kind === b.kind && a.id === b.id;
}

// ---------------------------------------------------------------- events

function handle(e: MuxEvent) {
  // Prompts, notices and port offers are the main window's job.
  if (POPOUT && (e.event === "prompt" || e.event === "notice" || e.event === "portDetected")) return;
  switch (e.event) {
    case "hosts":
      set({ hosts: e.hosts, folders: e.folders });
      break;
    case "connection":
      set((s) => ({ conn: { ...s.conn, [e.hostId]: { state: e.state, message: e.message } } }));
      if (e.state === "error" && e.message) toast(e.message, "error");
      break;
    case "sessions":
      set((s) => ({ servers: { ...s.servers, [e.hostId]: { tmux: e.tmux, sessions: e.sessions } } }));
      break;
    case "attached":
      set((s) => ({
        attachments: {
          ...s.attachments,
          [e.attachment]: s.attachments[e.attachment] ?? { id: e.attachment, hostId: e.hostId, state: null, live: true },
        },
      }));
      // Each window shows only its own: a pop-out window its attachment,
      // the main window everything except pop-outs.
      if (POPOUT ? e.attachment === POPOUT.attachment : !e.popout) openView({ kind: "att", id: e.attachment });
      break;
    case "sessionState":
      set((s) => {
        const prev = s.attachments[e.attachment];
        return {
          attachments: {
            ...s.attachments,
            [e.attachment]: { id: e.attachment, hostId: e.hostId, live: prev?.live ?? true, state: e.state },
          },
        };
      });
      saveWorkspace();
      break;
    case "paneSnapshot":
      getTerminal(paneKey(e.attachment, e.pane), e.cols, e.rows).reset(e.cols, e.rows, b64ToBytes(e.data));
      break;
    case "paneOutput":
      getTerminal(paneKey(e.attachment, e.pane)).write(b64ToBytes(e.data));
      break;
    case "attachmentStatus":
      set((s) => {
        const a = s.attachments[e.attachment];
        return a ? { attachments: { ...s.attachments, [e.attachment]: { ...a, live: e.live } } } : {};
      });
      break;
    case "attachmentClosed": {
      // This pop-out's tab moved back (or its session ended): close the window.
      if (POPOUT && e.attachment === POPOUT.attachment) {
        closeCurrentWindow();
        break;
      }
      // A pop-out window closed: its tab comes back here.
      if (get().popped[e.attachment]) {
        set((s) => {
          const { [e.attachment]: _gone, ...popped } = s.popped;
          return { popped };
        });
      }
      const name = get().attachments[e.attachment]?.state?.name;
      closeView({ kind: "att", id: e.attachment });
      set((s) => {
        const { [e.attachment]: _gone, ...rest } = s.attachments;
        return { attachments: rest };
      });
      disposeWhere((k) => k.startsWith(`a${e.attachment}:`));
      if (e.reason !== "detached") toast(`Session ${name ?? ""} closed: ${e.reason}`, "warn");
      saveWorkspace();
      break;
    }
    case "shellOutput":
      getTerminal(shellKey(e.shell)).write(b64ToBytes(e.data));
      break;
    case "shellClosed":
      set((s) => (s.shells[e.shell] ? { shells: { ...s.shells, [e.shell]: { ...s.shells[e.shell], closed: e.reason } } } : {}));
      peekTerminal(shellKey(e.shell))?.write(new TextEncoder().encode(`\r\n\x1b[2m[${e.reason}]\x1b[0m\r\n`));
      break;
    case "prompt":
      set((s) => ({ prompts: [...s.prompts, { prompt: e.prompt, hostId: e.hostId, request: e.request }] }));
      break;
    case "notice":
      toast(e.message, e.level === "warn" ? "warn" : e.level === "error" ? "error" : "info");
      break;
    case "tunnels":
      set({ tunnels: e.tunnels });
      break;
    case "vault":
      set({ vault: e.status });
      break;
    case "portDetected": {
      const d: DetectedPort = { hostId: e.hostId, port: e.port, path: e.path };
      set((s) =>
        s.detected.some((x) => x.hostId === d.hostId && x.port === d.port) ? {} : { detected: [...s.detected, d].slice(-4) },
      );
      // Offers go stale: the port stays one click away in the Ports dialog.
      setTimeout(() => dismissDetected(d.hostId, d.port), 120_000);
      break;
    }
  }
}

backend.onEvent(handle);

export function dismissDetected(hostId: HostId, port: number) {
  set((s) => ({ detected: s.detected.filter((x) => !(x.hostId === hostId && x.port === port)) }));
}

/** Which host a terminal ("a<att>:<pane>" / "s<shell>") belongs to. */
export function hostForTerminal(key: string): HostId | null {
  const s = get();
  const m = /^a(\d+):/.exec(key);
  if (m) return s.attachments[Number(m[1])]?.hostId ?? null;
  const sh = /^s(\d+)$/.exec(key);
  if (sh) return s.shells[Number(sh[1])]?.hostId ?? null;
  return null;
}

// ---------------------------------------------------------------- views

export function openView(v: View) {
  set((s) => ({
    views: s.views.some((x) => sameView(x, v)) ? s.views : [...s.views, v],
    active: v,
  }));
}

export function closeView(v: View) {
  set((s) => {
    const views = s.views.filter((x) => !sameView(x, v));
    let active = s.active;
    if (sameView(active, v)) {
      const i = s.views.findIndex((x) => sameView(x, v));
      active = views[Math.min(i, views.length - 1)] ?? null;
    }
    return { views, active };
  });
}

export function activeAttachment(): Attachment | null {
  const a = get().active;
  return a?.kind === "att" ? get().attachments[a.id] ?? null : null;
}

// ------------------------------------------------- workspace persistence

interface SavedWorkspace {
  open: { hostId: HostId; session: string; name: string }[];
  active: number;
}

const WS_KEY = "muxscape.workspace.v1";

function saveWorkspace() {
  if (POPOUT) return; // only the main window's tabs are restored on start
  const s = get();
  const open = s.views
    .filter((v): v is { kind: "att"; id: number } => v.kind === "att")
    .map((v) => s.attachments[v.id])
    .filter((a) => a?.state?.id)
    .map((a) => ({ hostId: a.hostId, session: a.state!.id!, name: a.state!.name }));
  const activeIdx = s.active?.kind === "att" ? open.findIndex((o) => s.attachments[(s.active as any).id]?.state?.id === o.session) : 0;
  try {
    localStorage.setItem(WS_KEY, JSON.stringify({ open, active: Math.max(activeIdx, 0) } satisfies SavedWorkspace));
  } catch {
    /* storage unavailable */
  }
}

export function loadSavedWorkspace(): SavedWorkspace | null {
  try {
    const raw = localStorage.getItem(WS_KEY);
    return raw ? (JSON.parse(raw) as SavedWorkspace) : null;
  } catch {
    return null;
  }
}
