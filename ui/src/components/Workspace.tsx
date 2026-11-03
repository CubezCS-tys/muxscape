import { useEffect, useLayoutEffect, useRef, useState } from "react";
import * as A from "../actions";
import { clipboard } from "../backend";
import { dividers, leaves, type Divider } from "../layout";
import { handleShortcut } from "../shortcuts";
import { useStore, type Attachment } from "../store";
import { cellSize, getTerminal, paneKey, shellKey, type CellSize } from "../terminals";
import type { WindowState } from "../types";

const PAD = 12; // px around the terminal grid

/** Copy on right-click when there's a selection, paste otherwise. */
async function rightClick(e: React.MouseEvent, key: string) {
  e.preventDefault();
  const t = getTerminal(key);
  if (t.term.hasSelection()) {
    await clipboard.write(t.term.getSelection());
    t.term.clearSelection();
  } else {
    const text = await clipboard.read();
    if (text) t.term.paste(text);
  }
  t.focus();
}

function PaneSlot({
  att, pane, cell, x, y, width, height, active, multi,
}: {
  att: Attachment; pane: string; cell: CellSize; x: number; y: number; width: number; height: number; active: boolean; multi: boolean;
}) {
  const host = useRef<HTMLDivElement>(null);
  const key = paneKey(att.id, pane);

  useLayoutEffect(() => {
    const t = getTerminal(key, width, height);
    t.handlers = {
      onData: (d) => A.sendInput(att.id, pane, d),
      onBinary: (d) => A.sendInput(att.id, pane, d, true),
      onFocus: () => {
        const w = A.activeWindow();
        if (w && w.active_pane !== pane) A.selectPane(pane);
      },
      onKey: handleShortcut,
    };
    if (host.current) t.mount(host.current);
  }, [key, att.id, pane]);

  useLayoutEffect(() => {
    getTerminal(key).resize(width, height);
  }, [key, width, height]);

  // Follow tmux's active pane with keyboard focus (unless the user is typing
  // somewhere else, like a rename field).
  useEffect(() => {
    if (!active) return;
    const el = document.activeElement;
    const typingElsewhere = el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA") && !el.classList.contains("xterm-helper-textarea");
    if (!typingElsewhere && !document.querySelector(".modal")) getTerminal(key).focus();
  }, [active, key]);

  return (
    <div
      className={`pane ${active && multi ? "active" : ""}`}
      style={{ left: x * cell.w, top: y * cell.h, width: width * cell.w, height: height * cell.h }}
      onContextMenu={(e) => rightClick(e, key)}
      data-testid={`pane-${pane}`}
    >
      <div ref={host} className="pane-term" />
    </div>
  );
}

function DividerBar({ d, cell, window }: { d: Divider; cell: CellSize; window: string }) {
  const [drag, setDrag] = useState<number | null>(null);
  const col = d.orientation === "col";
  const start = (e: React.PointerEvent) => {
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const origin = col ? e.clientX : e.clientY;
    const unit = col ? cell.w : cell.h;
    let delta = 0;
    setDrag(0);
    const move = (ev: PointerEvent) => {
      delta = Math.round(((col ? ev.clientX : ev.clientY) - origin) / unit);
      setDrag(delta);
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      setDrag(null);
      if (delta) A.moveDivider(window, d.path, d.index, delta);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };
  const offset = drag ?? 0;
  return (
    <div
      className={`divider ${col ? "col" : "row"} ${drag !== null ? "dragging" : ""}`}
      style={{
        left: (d.x + (col ? offset : 0)) * cell.w,
        top: (d.y + (col ? 0 : offset)) * cell.h,
        width: d.width * cell.w,
        height: d.height * cell.h,
      }}
      onPointerDown={start}
      data-testid={`divider-${d.path.join(".")}-${d.index}`}
    />
  );
}

function WindowView({ att, win, cell }: { att: Attachment; win: WindowState; cell: CellSize }) {
  const layout = win.visible_layout;
  const ls = leaves(layout);
  const ds = win.zoomed ? [] : dividers(layout);
  const activePane = win.active_pane ?? ls[0]?.pane;
  return (
    <div className={`window-grid ${ls.length > 1 ? "split" : ""}`} style={{ width: layout.width * cell.w, height: layout.height * cell.h }}>
      {ls.map((l) => (
        <PaneSlot key={l.pane} att={att} cell={cell} {...l} active={l.pane === activePane} multi={ls.length > 1 || win.zoomed} />
      ))}
      {ds.map((d) => (
        <DividerBar key={`${d.path.join(".")}-${d.index}`} d={d} cell={cell} window={win.id} />
      ))}
    </div>
  );
}

function ShellViewPane({ shell, cell }: { shell: number; cell: CellSize }) {
  const host = useRef<HTMLDivElement>(null);
  const { cols, rows } = useStore((s) => s.viewport);
  const key = shellKey(shell);
  useLayoutEffect(() => {
    const t = getTerminal(key, cols, rows);
    t.handlers = {
      onData: (d) => A.shellInput(shell, d),
      onBinary: (d) => A.shellInput(shell, d, true),
      onKey: handleShortcut,
    };
    if (host.current) t.mount(host.current);
    t.focus();
  }, [key, shell]);
  useLayoutEffect(() => {
    getTerminal(key).resize(cols, rows);
    A.shellResize(shell, cols, rows);
  }, [key, shell, cols, rows]);
  return (
    <div className="window-grid" style={{ width: cols * cell.w, height: rows * cell.h }}>
      <div className="pane" style={{ left: 0, top: 0, width: cols * cell.w, height: rows * cell.h }} onContextMenu={(e) => rightClick(e, key)}>
        <div ref={host} className="pane-term" />
      </div>
    </div>
  );
}

export function Workspace() {
  const ref = useRef<HTMLDivElement>(null);
  const active = useStore((s) => s.active);
  const att = useStore((s) => (s.active?.kind === "att" ? s.attachments[s.active.id] : undefined));
  const viewport = useStore((s) => s.viewport);
  const [cell, setCell] = useState<CellSize>(() => cellSize());
  const fontSize = useStore((s) => s.prefs.fontSize);

  // Measure the area and derive the tmux client size in cells.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const c = cellSize();
      setCell(c);
      const cols = Math.max(20, Math.floor((el.clientWidth - PAD * 2) / c.w));
      const rows = Math.max(5, Math.floor((el.clientHeight - PAD * 2) / c.h));
      const v = useStore.getState().viewport;
      if (v.cols !== cols || v.rows !== rows) useStore.setState({ viewport: { cols, rows } });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [fontSize]); // text size changes the cell size: re-measure

  // Tell tmux (debounced) whenever the viewport or active session changes.
  useEffect(() => {
    if (!att) return;
    const t = setTimeout(() => A.resizeClient(att.id, viewport.cols, viewport.rows), 60);
    return () => clearTimeout(t);
  }, [att?.id, viewport.cols, viewport.rows]);

  const win = att?.state?.windows.find((w) => w.active);
  return (
    <div className="workspace" ref={ref} style={{ padding: PAD }}>
      {att && win && <WindowView key={`${att.id}:${win.id}`} att={att} win={win} cell={cell} />}
      {att && !att.state && <div className="placeholder">Attaching…</div>}
      {active?.kind === "shell" && <ShellViewPane key={active.id} shell={active.id} cell={cell} />}
      {!active && <Welcome />}
      {att && !att.live && (
        <div className="offline-banner">
          <span className="spinner" /> Connection lost — reconnecting. Your tmux session keeps running on the server.
        </div>
      )}
    </div>
  );
}

function Welcome() {
  return (
    <div className="welcome">
      <h1>Muxscape</h1>
      <p className="tagline">The SSH workspace that never closes.</p>
      <ul>
        <li>Pick a server on the left — double-click opens its tmux workspace.</li>
        <li>Every tab is a real tmux window; every pane a real tmux pane.</li>
        <li>Close the app any time: your sessions keep running on the server.</li>
      </ul>
      <button className="primary" onClick={() => useStore.setState({ hostDialog: "new" })}>Add a host</button>
    </div>
  );
}
