// User intents -> core requests. Every structural change goes to tmux; the
// UI only changes when tmux reports back (tmux is the source of truth).

import { backend, binaryStrToB64, strToB64 } from "./backend";
import { leaves, neighbor, type Direction } from "./layout";
import { activeAttachment, loadSavedWorkspace, openView, toast, useStore } from "./store";
import type { AttachTarget, Host, HostId, Request, SessionInfo } from "./types";
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

export function cycleWindow(delta: number) {
  const ws = att()?.state?.windows ?? [];
  const i = ws.findIndex((w) => w.active);
  if (ws.length && i >= 0) selectWindow(ws[(i + delta + ws.length) % ws.length].id);
}

export function selectWindowAt(pos: number) {
  const ws = att()?.state?.windows ?? [];
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
