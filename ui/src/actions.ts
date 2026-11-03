// User intents -> core requests. Every structural change goes to tmux; the
// UI only changes when tmux reports back (tmux is the source of truth).

import { backend, binaryStrToB64, bytesToB64, clipboard, closeCurrentWindow, externalOpener, strToB64 } from "./backend";
import { leaves, neighbor, type Direction } from "./layout";
import { POPOUT, visibleWindows, activeAttachment, dismissDetected, hostForTerminal, loadSavedWorkspace, openView, probeKey, toast, useStore, type PortInfo } from "./store";
import { setFileHandler, setLinkHandler, type PastedFile } from "./terminals";
import type { AttachTarget, Host, HostId, ListeningPort, PaneRef, PortProbe, Request, SessionInfo, TunnelInfo, VaultStatus } from "./types";
import { askConfirm, askText } from "./dialogs";

const get = useStore.getState;
const set = useStore.setState;

async function req<T = any>(r: Request, quiet = false): Promise<T | undefined> {
  try {
    return await backend.request<T>(r);
  } catch (e) {
    if (!quiet) toast(String(e), "error");
    return undefined;
  }
}

export async function init() {
  const r = await req<{ hosts: Host[]; folders: string[]; states: Record<string, string> }>({ cmd: "listHosts" });
  if (!r) return;
  const conn: Record<string, { state: any; message: null }> = {};
  for (const [id, st] of Object.entries(r.states ?? {})) conn[id] = { state: st, message: null };
  set({ hosts: r.hosts, folders: r.folders, conn });
  const tunnels = await req<TunnelInfo[]>({ cmd: "listTunnels" }, true);
  if (tunnels) set({ tunnels });
  const vault = await req<VaultStatus>({ cmd: "vaultStatus" }, true);
  if (vault) set({ vault });
  setLinkHandler(openLink);
  setFileHandler(uploadToTerminal);
  if (POPOUT) {
    // A pop-out window: show the session it was opened for. When the window
    // goes away, its hidden companion session is removed (the desktop app
    // also does this from Rust, in case this never runs).
    window.addEventListener("pagehide", () => {
      backend.request({ cmd: "closePopout", attachment: POPOUT!.attachment }).catch(() => {});
    });
    await attach(POPOUT.hostId, { kind: "existing", session: POPOUT.session });
    return;
  }
  await restoreWorkspace();
}

// ------------------------------------------------------------- hosts

export async function saveHost(h: Host) {
  const saved = await req<Host>({ cmd: "saveHost", host: h });
  if (saved) set({ hostDialog: null });
  return saved;
}

export async function deleteHost(id: HostId) {
  await req({ cmd: "deleteHost", hostId: id });
}

export async function connect(hostId: HostId) {
  set((s) => ({ expanded: { ...s.expanded, [hostId]: true } }));
  return req<{ tmux: { kind: string }; sessions: SessionInfo[] }>({ cmd: "connect", hostId });
}

export async function disconnect(hostId: HostId) {
  await req({ cmd: "disconnect", hostId });
}

export async function refreshSessions(hostId: HostId) {
  await req({ cmd: "refreshSessions", hostId }, true);
}

/** Connect (if needed) and open the most relevant session, or a shell. */
export async function quickOpen(hostId: HostId) {
  const r = await connect(hostId);
  if (!r) return;
  if (r.tmux.kind === "notInstalled") {
    toast("tmux is not installed on this server — opening a plain shell");
    return openShell(hostId);
  }
  return attach(hostId, { kind: "auto" });
}

// ---------------------------------------------------------- sessions

export async function attach(hostId: HostId, target: AttachTarget) {
  const { cols, rows } = get().viewport;
  const r = await req<{ attachment: number }>({ cmd: "attach", hostId, target, cols, rows });
  if (r) openView({ kind: "att", id: r.attachment });
  return r?.attachment;
}

export async function newSession(hostId: HostId) {
  const name = await askText("New tmux session", "work", { message: "Letters, digits, - and _ (you can rename it to anything later).", ok: "Create" });
  if (name === null) return;
  await attach(hostId, { kind: "new", name: name || "main" });
}

export async function killSession(hostId: HostId, s: SessionInfo) {
  const ok = await askConfirm("Kill session", `Kill tmux session "${s.name}"? All ${s.windows} window(s) and the processes in them will be terminated.`, { danger: true, ok: "Kill session" });
  if (!ok) return;
  await req({ cmd: "killSession", hostId, session: s.id });
}

export async function detach(att: number) {
  await req({ cmd: "detach", attachment: att });
}

export async function renameSession(att: number, current: string) {
  const name = await askText("Rename session", current, { ok: "Rename" });
  if (name && name !== current) await req({ cmd: "renameSession", attachment: att, name });
}

async function restoreWorkspace() {
  const saved = loadSavedWorkspace();
  if (!saved?.open.length) return;
  const byHost = new Map<string, typeof saved.open>();
  for (const o of saved.open) {
    if (!get().hosts.some((h) => h.id === o.hostId)) continue;
    byHost.set(o.hostId, [...(byHost.get(o.hostId) ?? []), o]);
  }
  for (const [hostId, items] of byHost) {
    const r = await connect(hostId);
    if (!r) continue;
    for (const it of items) {
      // Ids can be reused after a server restart: require the name to match,
      // else fall back to a session with the same name.
      const s = r.sessions.find((x) => x.id === it.session && x.name === it.name) ?? r.sessions.find((x) => x.name === it.name);
      if (s) await attach(hostId, { kind: "existing", session: s.id });
    }
  }
}

// ------------------------------------------------------- windows/panes

function att() {
  return activeAttachment();
}

export function activeWindow() {
  return att()?.state?.windows.find((w) => w.active) ?? null;
}

export function activePane(): string | null {
  const w = activeWindow();
  if (!w) return null;
  return w.active_pane ?? (leaves(w.visible_layout)[0]?.pane ?? null);
}

export async function selectWindow(id: string) {
  const a = att();
  if (a) await req({ cmd: "selectWindow", attachment: a.id, window: id });
}

export async function newWindow() {
  if (POPOUT) return; // a pop-out holds exactly one tab
  const a = att();
  if (a) await req({ cmd: "newWindow", attachment: a.id });
}

export async function killWindow(id: string, ask = true) {
  const a = att();
  const w = a?.state?.windows.find((x) => x.id === id);
  if (!a || !w) return;
  if (ask && !(await askConfirm("Close window", `Close window "${w.name}"? Processes running in it will be terminated.`, { danger: true, ok: "Close window" }))) return;
  await req({ cmd: "killWindow", attachment: a.id, window: id });
}

export async function renameWindow(id: string, name: string) {
  const a = att();
  if (a) await req({ cmd: "renameWindow", attachment: a.id, window: id, name });
}

/** The tabs shown in this window (see visibleWindows). */
function tabs() {
  const a = att();
  return a?.state ? visibleWindows(a.id, a.state.windows) : [];
}

export function cycleWindow(delta: number) {
  const ws = tabs();
  const i = ws.findIndex((w) => w.active);
  if (ws.length && i >= 0) selectWindow(ws[(i + delta + ws.length) % ws.length].id);
}

export function selectWindowAt(pos: number) {
  const ws = tabs();
  const w = pos === 9 ? ws[ws.length - 1] : ws[pos - 1];
  if (w) selectWindow(w.id);
}

export async function selectPane(pane: string) {
  const a = att();
  if (a) await req({ cmd: "selectPane", attachment: a.id, pane }, true);
}

export async function split(horizontal: boolean, pane = activePane()) {
  const a = att();
  if (a && pane) await req({ cmd: "splitPane", attachment: a.id, pane, horizontal });
}

export async function killPane(pane = activePane(), ask = true) {
  const a = att();
  if (!a || !pane) return;
  if (ask && !(await askConfirm("Close pane", "Close this pane? Its process will be terminated.", { danger: true, ok: "Close pane" }))) return;
  await req({ cmd: "killPane", attachment: a.id, pane });
}

export async function zoomPane(pane = activePane()) {
  const a = att();
  if (a && pane) await req({ cmd: "zoomPane", attachment: a.id, pane });
}

export function focusDirection(dir: Direction) {
  const w = activeWindow();
  const p = activePane();
  if (!w || !p) return;
  const n = neighbor(leaves(w.visible_layout), p, dir);
  if (n) selectPane(n);
}

export async function moveDivider(window: string, path: number[], index: number, delta: number) {
  const a = att();
  if (a && delta) await req({ cmd: "moveDivider", attachment: a.id, window, path, index, delta });
}

export function sendInput(attachment: number, pane: string, data: string, binary = false) {
  backend.send({ cmd: "input", attachment, pane, data: binary ? binaryStrToB64(data) : strToB64(data) });
}

export function resizeClient(attachment: number, cols: number, rows: number) {
  backend.send({ cmd: "resize", attachment, cols, rows });
}

// ------------------------------------------------------------- shells

export async function openShell(hostId: HostId) {
  const { cols, rows } = get().viewport;
  const r = await req<{ shell: number }>({ cmd: "openShell", hostId, cols, rows });
  if (!r) return;
  set((s) => ({ shells: { ...s.shells, [r.shell]: { id: r.shell, hostId, closed: null } } }));
  openView({ kind: "shell", id: r.shell });
}

export function shellInput(shell: number, data: string, binary = false) {
  backend.send({ cmd: "shellInput", shell, data: binary ? binaryStrToB64(data) : strToB64(data) });
}

export function shellResize(shell: number, cols: number, rows: number) {
  backend.send({ cmd: "shellResize", shell, cols, rows });
}

export async function closeShell(shell: number) {
  await req({ cmd: "closeShell", shell }, true);
}

// ------------------------------------------------------------- prompts

export async function answerPrompt(prompt: number, answer: import("./types").PromptAnswer) {
  set((s) => ({ prompts: s.prompts.filter((p) => p.prompt !== prompt) }));
  await req({ cmd: "answerPrompt", prompt, answer });
}

// ------------------------------------------------------- port forwarding

const hostLabel = (id: HostId) => get().hosts.find((h) => h.id === id)?.label ?? id;

export function tunnelUrl(t: TunnelInfo, scheme = "http:") {
  return `${scheme}//localhost:${t.localPort}${t.path ?? "/"}`;
}

/**
 * Forward `remotePort` on the host to this machine (reusing an existing
 * forward) and, by default, open it in the browser. Call straight from a
 * click handler so the browser tab isn't treated as a popup.
 */
export async function forwardPort(
  hostId: HostId,
  remotePort: number,
  opts: { path?: string | null; open?: boolean; pinned?: boolean; localPort?: number | null; scheme?: string } = {},
) {
  const open = opts.open ?? true;
  const go = open ? externalOpener() : null;
  const t = await req<TunnelInfo>({
    cmd: "openTunnel", hostId, remotePort, path: opts.path ?? null, pinned: !!opts.pinned, localPort: opts.localPort ?? null,
  });
  dismissDetected(hostId, remotePort);
  if (!t) {
    go?.(null);
    return undefined;
  }
  go?.(tunnelUrl(t, opts.scheme ?? (probeFor(hostId, remotePort)?.tls ? "https:" : "http:")));
  if (t.localPort !== remotePort) {
    toast(`Port ${remotePort} is busy on this machine — ${hostLabel(hostId)}:${remotePort} is on localhost:${t.localPort}`);
  } else if (!open) {
    toast(`Forwarding localhost:${t.localPort} → ${hostLabel(hostId)}:${remotePort}`);
  }
  return t;
}

/** The last probe of a host's port, if any. */
export function probeFor(hostId: HostId, port: number): PortProbe | undefined {
  const info = get().portInfo[hostId];
  if (!info) return undefined;
  const lp = info.ports.find((p) => p.port === port);
  return info.probes[probeKey(port, lp?.pid)];
}

export function openTunnel(t: TunnelInfo) {
  externalOpener()(tunnelUrl(t, probeFor(t.hostId, t.remotePort)?.tls ? "https:" : "http:"));
}

export async function copyAddress(t: TunnelInfo) {
  await clipboard.write(`localhost:${t.localPort}`);
  toast(`Copied localhost:${t.localPort}`);
}

export async function copyTunnelUrl(t: TunnelInfo) {
  await clipboard.write(tunnelUrl(t));
  toast(`Copied ${tunnelUrl(t)}`);
}

export async function closeTunnel(t: TunnelInfo) {
  await req({ cmd: "closeTunnel", tunnel: t.id });
}

export async function pinTunnel(t: TunnelInfo, pinned: boolean) {
  await req({ cmd: "pinTunnel", tunnel: t.id, pinned });
}

/**
 * Rescan a host's listening ports, then probe the ones not probed yet (a
 * probe is kept while the same process owns the port).
 */
export async function refreshPorts(hostId: HostId) {
  const prev = get().portInfo[hostId];
  if (prev?.scanning) return;
  const patch = (p: Partial<PortInfo>) =>
    set((s) => ({
      portInfo: {
        ...s.portInfo,
        [hostId]: { ...((s.portInfo[hostId] as PortInfo | undefined) ?? { ports: [], probes: {}, scannedAt: 0, scanning: false }), ...p },
      },
    }));
  patch({ scanning: true });
  const r = await req<{ ports: ListeningPort[] }>({ cmd: "scanPorts", hostId }, true);
  if (!r) return patch({ scanning: false });
  patch({ ports: r.ports, scannedAt: Date.now(), scanning: false });
  // Forwarded ports are probed too, even if the scan can't see them.
  const tunnelPorts = get().tunnels.filter((t) => t.hostId === hostId).map((t) => t.remotePort);
  const probes = get().portInfo[hostId]?.probes ?? {};
  const targets = [
    ...r.ports.map((p) => ({ port: p.port, process: p.process, pid: p.pid })),
    ...tunnelPorts.filter((p) => !r.ports.some((x) => x.port === p)).map((p) => ({ port: p, process: null, pid: null })),
  ].filter((t) => !probes[probeKey(t.port, t.pid)]);
  if (!targets.length) return;
  const pr = await req<{ probes: PortProbe[] }>(
    { cmd: "probePorts", hostId, targets: targets.map(({ port, process }) => ({ port, process })) },
    true,
  );
  if (!pr) return;
  const next = { ...(get().portInfo[hostId]?.probes ?? {}) };
  for (const p of pr.probes) {
    const t = targets.find((x) => x.port === p.port);
    next[probeKey(p.port, t?.pid)] = p;
  }
  patch({ probes: next });
}

/** Re-probe one port (e.g. its service restarted). */
export async function reprobe(hostId: HostId, port: number) {
  const info = get().portInfo[hostId];
  const lp = info?.ports.find((p) => p.port === port);
  const pr = await req<{ probes: PortProbe[] }>({ cmd: "probePorts", hostId, targets: [{ port, process: lp?.process ?? null }] }, true);
  const p = pr?.probes[0];
  if (!p || !info) return;
  set((s) => ({
    portInfo: { ...s.portInfo, [hostId]: { ...s.portInfo[hostId], probes: { ...s.portInfo[hostId].probes, [probeKey(port, lp?.pid)]: p } } },
  }));
}

/** Host the Ports panel shows: the chosen one, else the active view's. */
export function activeHostId(): HostId | null {
  const s = get();
  if (s.active?.kind === "shell") return s.shells[s.active.id]?.hostId ?? null;
  if (s.active?.kind === "att") return s.attachments[s.active.id]?.hostId ?? null;
  return null;
}

export function openPorts(hostId?: HostId | null) {
  set({ portsOpen: true, portsHost: hostId ?? get().portsHost ?? activeHostId() ?? get().hosts[0]?.id ?? null });
}

export function togglePorts(hostId?: HostId | null) {
  if (get().portsOpen && (!hostId || hostId === get().portsHost)) set({ portsOpen: false });
  else openPorts(hostId);
}

/** Show the tmux pane a port's process runs in. */
export async function jumpToPane(hostId: HostId, pane: PaneRef) {
  const a = await attach(hostId, { kind: "existing", session: pane.sessionId });
  if (!a) return;
  await req({ cmd: "selectWindow", attachment: a, window: pane.windowId }, true);
  await req({ cmd: "selectPane", attachment: a, pane: pane.paneId }, true);
}

const LOCAL_HOSTS = new Set(["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"]);

/**
 * A link clicked in a terminal. `http://localhost:PORT/...` printed on a
 * server means *that server's* localhost: forward it, then open it here.
 */
export function openLink(termKey: string, uri: string) {
  const hostId = hostForTerminal(termKey);
  try {
    const u = new URL(uri);
    if (hostId && (u.protocol === "http:" || u.protocol === "https:") && LOCAL_HOSTS.has(u.hostname) && u.port) {
      const path = u.pathname + u.search + u.hash;
      forwardPort(hostId, Number(u.port), { path: path === "/" ? null : path, scheme: u.protocol });
      return;
    }
  } catch {
    /* not a URL we understand: let the browser try */
  }
  externalOpener()(uri);
}

// ------------------------------------------------------------ password vault
// Secrets go to the core and never come back: the UI only sees status.

/** Throws the core's message (shown inline by the forms) instead of toasting. */
async function vaultReq(r: Request): Promise<void> {
  await backend.request(r);
}

export const vaultCreate = (password: string, remember: boolean) => vaultReq({ cmd: "vaultCreate", password, remember });
export const vaultUnlock = (password: string, remember: boolean) => vaultReq({ cmd: "vaultUnlock", password, remember });
export const vaultChangePassword = (old: string, nw: string) => vaultReq({ cmd: "vaultChangePassword", old, new: nw });
export const vaultSetRemember = (remember: boolean) => vaultReq({ cmd: "vaultSetRemember", remember });

export async function vaultLock() {
  await req({ cmd: "vaultLock" });
  toast("Vault locked");
}

export async function vaultForget(slot: string) {
  await req({ cmd: "vaultForget", slot });
}

export async function vaultReset() {
  const ok = await askConfirm(
    "Reset vault",
    "Delete the vault and every saved password? This can't be undone. Your hosts are kept; you'll just be asked for passwords again.",
    { danger: true, ok: "Delete saved passwords" },
  );
  if (ok) await req({ cmd: "vaultReset" });
  return ok;
}

/** "password:dev@host:22" → { kind: "Password", label: "dev@host:22" } */
export function describeSlot(slot: string): { kind: string; label: string } {
  const i = slot.indexOf(":");
  const kind = slot.slice(0, i);
  return { kind: kind === "passphrase" ? "Key passphrase" : "Password", label: slot.slice(i + 1) };
}

/** The vault slot a host's password would use (as the core computes it). */
export function hostPasswordSlot(h: Host): string | null {
  if (!h.user) return null;
  return `password:${h.user}@${h.hostname}${h.port === 22 ? "" : `:${h.port}`}`;
}

// ------------------------------------------------------------ pop-out windows

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/**
 * Show a tab in its own window. tmux's current window belongs to the session,
 * so the core makes a hidden companion session (same windows, own current
 * window) and the new window attaches to that.
 */
export async function popOut(windowId: string, at?: { x: number; y: number }) {
  const a = att();
  const sessionId = a?.state?.id;
  if (!a || !sessionId || POPOUT) return;
  if (tabs().length <= 1) {
    toast("That's the only tab here: nothing to move it away from");
    return;
  }
  const w = a.state!.windows.find((x) => x.id === windowId);
  const { cols, rows } = get().viewport;
  const r = await req<{ attachment: number; session: string }>({ cmd: "popOut", hostId: a.hostId, session: sessionId, window: windowId, cols, rows });
  if (!r) return;
  const host = get().hosts.find((h) => h.id === a.hostId);
  const title = `${host?.label ?? a.hostId} › ${w?.name ?? "window"}`;
  const q = new URLSearchParams(location.search);
  q.set("popout", String(r.attachment));
  q.set("host", a.hostId);
  q.set("session", r.session);
  q.set("name", a.state!.name);
  q.set("window", windowId);
  const width = 1100;
  const height = 720;
  if (isTauri) {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const win = new WebviewWindow(`popout-${r.attachment}`, {
      url: `index.html?${q}`,
      // Native file-drop handling would swallow HTML5 drag & drop (tabs).
      dragDropEnabled: false,
      title,
      width,
      height,
      ...(at ? { x: Math.max(0, at.x - 120), y: Math.max(0, at.y - 20) } : {}),
    });
    win.once("tauri://error", (e) => {
      toast(`Could not open a window: ${String(e.payload)}`, "error");
      req({ cmd: "closePopout", attachment: r.attachment }, true);
    });
  } else {
    const pos = at ? `,left=${Math.max(0, at.x - 120)},top=${Math.max(0, at.y - 20)}` : "";
    const win = window.open(`${location.pathname}?${q}`, `popout-${r.attachment}`, `popup,width=${width},height=${height}${pos}`);
    if (!win) {
      toast("The browser blocked the new window", "warn");
      req({ cmd: "closePopout", attachment: r.attachment }, true);
    }
  }
  // The tab moves to the new window: hide it here, and if it was showing,
  // switch to another one (two windows on one tab would fight over its size).
  set((s) => ({ popped: { ...s.popped, [r.attachment]: { from: a.id, window: windowId } } }));
  const now = get().attachments[a.id]?.state;
  if (now?.windows.find((x) => x.active)?.id === windowId) {
    const next = visibleWindows(a.id, now.windows)[0];
    if (next) selectWindow(next.id);
  }
}

/** Move a popped-out tab back into this (main) window and show it. */
export async function moveBack(popoutAttachment: number) {
  const p = get().popped[popoutAttachment];
  await req({ cmd: "closePopout", attachment: popoutAttachment });
  if (p && get().active?.kind === "att" && (get().active as { id: number }).id === p.from) selectWindow(p.window);
}

/** From a pop-out: send its tab back (the window then closes itself). */
export async function moveBackFromPopout() {
  if (POPOUT) await req({ cmd: "closePopout", attachment: POPOUT.attachment });
}

/** A pop-out stays on its tab, and closes when that tab is closed. */
export function keepPopoutOnItsTab() {
  const pop = POPOUT;
  if (!pop) return;
  const a = att();
  const ws = a?.state?.windows;
  if (!a || !ws) return;
  if (!ws.some((w) => w.id === pop.window)) {
    closeThisWindow();
  } else if (!ws.find((w) => w.active && w.id === pop.window)) {
    selectWindow(pop.window);
  }
}

function closeThisWindow() {
  closeCurrentWindow();
}

// ------------------------------------------------- pasting files / screenshots

/** Type text into a terminal (pane or plain shell) as if typed. */
function typeInto(termKey: string, text: string) {
  const m = /^a(\d+):(.+)$/.exec(termKey);
  if (m) return sendInput(Number(m[1]), m[2], text);
  const sh = /^s(\d+)$/.exec(termKey);
  if (sh) shellInput(Number(sh[1]), text);
}

const fmtSize = (n: number) => (n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);

/**
 * Files pasted or dropped on a terminal: copy them to that server and type
 * their paths, so a remote agent (Claude Code…) can open them.
 */
export async function uploadToTerminal(termKey: string, files: PastedFile[]) {
  const hostId = hostForTerminal(termKey);
  if (!hostId || !files.length) return;
  const total = files.reduce((n, f) => n + f.bytes.length, 0);
  toast(`Uploading ${files.length === 1 ? files[0].name : `${files.length} files`} (${fmtSize(total)})…`);
  const paths: string[] = [];
  for (const f of files) {
    const r = await req<{ path: string }>({ cmd: "upload", hostId, name: f.name, data: bytesToB64(f.bytes) });
    if (!r) return;
    paths.push(r.path);
  }
  typeInto(termKey, paths.join(" ") + " ");
  toast(`Uploaded to ${paths.length === 1 ? paths[0] : `~/.cache/muxscape/uploads (${paths.length})`}`);
}

/** Paste from the menu / Ctrl+Shift+V: an image if there is one, else text. */
export async function pasteInto(termKey: string, pasteText: (t: string) => void) {
  const text = await clipboard.read();
  if (!text) {
    const png = await clipboard.readImage();
    if (png) return uploadToTerminal(termKey, [{ name: "image.png", bytes: png }]);
  }
  if (text) pasteText(text);
}
