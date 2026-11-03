// Optional keyboard shortcuts. All use Ctrl+Shift / Alt combos that
// terminal programs don't normally need, so vim, emacs, htop etc. keep
// every plain Ctrl key. No tmux prefix is ever required.

import * as A from "./actions";
import { clipboard } from "./backend";
import { activeAttachment, useStore } from "./store";
import { getTerminal, paneKey, shellKey } from "./terminals";

function activeTermKey(): string | null {
  const s = useStore.getState();
  if (s.active?.kind === "shell") return shellKey(s.active.id);
  const a = activeAttachment();
  const p = A.activePane();
  return a && p ? paneKey(a.id, p) : null;
}

/** Returns true if the event was a shortcut (and was handled). */
export function handleShortcut(e: KeyboardEvent): boolean {
  const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  const cs = (e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey;
  let handled = true;
  if (cs && k === "t") A.newWindow();
  else if (cs && k === "w") A.killPane();
  else if (cs && k === "d") A.split(true);
  else if (cs && k === "e") A.split(false);
  else if (cs && k === "z") A.zoomPane();
  else if (cs && k === "p") useStore.setState({ switcherOpen: true });
  else if (cs && k === "o") A.togglePorts();
  else if (cs && k === "c") {
    const key = activeTermKey();
    const sel = key ? getTerminal(key).term.getSelection() : "";
    if (sel) clipboard.write(sel);
  } else if (cs && k === "v") {
    const key = activeTermKey();
    if (key) A.pasteInto(key, (t) => getTerminal(key).term.paste(t));
  } else if (e.ctrlKey && !e.altKey && k === "Tab") A.cycleWindow(e.shiftKey ? -1 : 1);
  else if (e.ctrlKey && !e.shiftKey && !e.altKey && k === "PageDown") A.cycleWindow(1);
  else if (e.ctrlKey && !e.shiftKey && !e.altKey && k === "PageUp") A.cycleWindow(-1);
  else if (e.altKey && !e.ctrlKey && !e.shiftKey && /^[1-9]$/.test(e.key)) A.selectWindowAt(Number(e.key));
  else if (e.altKey && e.shiftKey && !e.ctrlKey && k === "ArrowLeft") A.focusDirection("left");
  else if (e.altKey && e.shiftKey && !e.ctrlKey && k === "ArrowRight") A.focusDirection("right");
  else if (e.altKey && e.shiftKey && !e.ctrlKey && k === "ArrowUp") A.focusDirection("up");
  else if (e.altKey && e.shiftKey && !e.ctrlKey && k === "ArrowDown") A.focusDirection("down");
  else handled = false;
  if (handled) {
    e.preventDefault();
    e.stopPropagation();
  }
  return handled;
}

export const SHORTCUTS: [string, string][] = [
  ["Ctrl+Shift+T", "New window (tab)"],
  ["Ctrl+Tab / Ctrl+PgDn", "Next window"],
  ["Ctrl+Shift+Tab / Ctrl+PgUp", "Previous window"],
  ["Alt+1…9", "Go to window (9 = last)"],
  ["Ctrl+Shift+P", "Go to window / session…"],
  ["Ctrl+Shift+O", "Ports & forwarding"],
  ["Ctrl+Shift+D", "Split right"],
  ["Ctrl+Shift+E", "Split down"],
  ["Alt+Shift+Arrows", "Focus neighbouring pane"],
  ["Ctrl+Shift+Z", "Zoom pane"],
  ["Ctrl+Shift+W", "Close pane"],
  ["Ctrl+Shift+C / V", "Copy / paste"],
  ["Right-click", "Copy selection, or paste"],
];
