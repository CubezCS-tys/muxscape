import { FindBar } from "./Find";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import * as A from "../actions";
import { clipboard } from "../backend";
import { dividers, leaves, type Divider } from "../layout";
import { handleShortcut } from "../shortcuts";
import { useStore, type Attachment } from "../store";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { cellSize, getTerminal, paneKey, shellKey, type CellSize } from "../terminals";
import type { WindowState } from "../types";

const PAD = 12; // px around the terminal grid

type Menu = { x: number; y: number; items: MenuItem[] } | null;

/** Drop files on a pane: upload them to the server and type their paths. */
function fileDrop(key: string, setDropping: (b: boolean) => void) {
  const hasFiles = (e: React.DragEvent) => e.dataTransfer.types.includes("Files");
  return {
    onDragOver: (e: React.DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
      setDropping(true);
    },
    onDragLeave: (e: React.DragEvent) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node)) setDropping(false);
    },
    onDrop: async (e: React.DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      setDropping(false);
      const files = await Promise.all(
        [...e.dataTransfer.files].map(async (f) => ({ name: f.name, bytes: new Uint8Array(await f.arrayBuffer()) })),
      );
      A.uploadToTerminal(key, files);
    },
  };
}

/** Copy / paste items for a terminal (the selection is taken now: the menu
 * is about what was selected when the user right-clicked). */
function clipboardItems(key: string): MenuItem[] {
  const t = getTerminal(key);
  const sel = t.term.hasSelection() ? t.term.getSelection() : "";
  return [
    ...(sel
      ? [{ label: "Copy", hint: "Ctrl+Shift+C", testid: "pane-copy", action: () => { clipboard.write(sel); t.term.clearSelection(); t.focus(); } }]
      : []),
    {
      label: "Paste",
      hint: "Ctrl+Shift+V",
      testid: "pane-paste",
      action: async () => {
        await A.pasteInto(key, (text) => t.term.paste(text));
        t.focus();
      },
    },
  ];
}

function PaneSlot({
  att, pane, cell, x, y, width, height, active, multi,
}: {
  att: Attachment; pane: string; cell: CellSize; x: number; y: number; width: number; height: number; active: boolean; multi: boolean;
}) {
  const host = useRef<HTMLDivElement>(null);
  const key = paneKey(att.id, pane);
  const [menu, setMenu] = useState<Menu>(null);
  const [dropping, setDropping] = useState(false);
  const zoomed = useStore((s) => !!s.attachments[att.id]?.state?.windows.find((w) => w.active)?.zoomed);
  const openMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    // Act on the pane that was clicked: focus it first (outline follows).
    A.selectPane(pane);
    const items: MenuItem[] = [
      ...clipboardItems(key),
      { label: "Split right", hint: "Ctrl+Shift+D", sep: true, action: () => A.split(true, pane) },
      { label: "Split down", hint: "Ctrl+Shift+E", action: () => A.split(false, pane) },
      ...(multi
        ? [
            { label: zoomed ? "Unzoom pane" : "Zoom pane", hint: "Ctrl+Shift+Z", testid: "pane-zoom", action: () => A.zoomPane(pane) },
            { label: "Close pane", hint: "Ctrl+Shift+W", danger: true, sep: true, testid: "pane-close", action: () => A.killPane(pane) },
          ]
        : []),
    ];
    setMenu({ x: e.clientX, y: e.clientY, items });
  };

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
      onContextMenu={openMenu}
      {...fileDrop(key, setDropping)}
      data-testid={`pane-${pane}`}
    >
      {dropping && <div className="pane-drop">Drop to upload to the server</div>}
      <div ref={host} className="pane-term" />
      {menu && <ContextMenu {...menu} onClose={() => setMenu(null)} />}
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
  const [menu, setMenu] = useState<Menu>(null);
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
      <div
        className="pane"
        style={{ left: 0, top: 0, width: cols * cell.w, height: rows * cell.h }}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY, items: clipboardItems(key) });
        }}
      >
        <div ref={host} className="pane-term" />
        {menu && <ContextMenu {...menu} onClose={() => setMenu(null)} />}
      </div>
    </div>
  );
}

export function Workspace() {
  const ref = useRef<HTMLDivElement>(null);
  const active = useStore((s) => s.active);
  const att = useStore((s) => (s.active?.kind === "att" ? s.attachments[s.active.id] : undefined));
  const viewport = useStore((s) => s.viewport);
  const findOpen = useStore((s) => s.findOpen);
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
      {findOpen && active && <FindBar key={JSON.stringify(active)} />}
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
