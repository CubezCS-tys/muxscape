// Persistent xterm.js instances, one per remote pane (and per plain shell).
//
// Terminals live outside React: switching tabs re-parents the existing DOM
// node instead of recreating anything, and pane output is written straight
// to the emulator without touching React state.

import { Terminal, type ITheme } from "@xterm/xterm";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SearchAddon } from "@xterm/addon-search";

export const FONT_FAMILY =
  '"JetBrains Mono", "Cascadia Mono", "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", "Liberation Mono", monospace';
export const FONT_SIZE = 13;
export const LINE_HEIGHT = 1.15;

export const THEME: ITheme = {
  background: "#0e1116",
  foreground: "#d6dbe4",
  cursor: "#8ab4ff",
  cursorAccent: "#0e1116",
  selectionBackground: "#3a4a6b",
  black: "#1b1f27", red: "#f07178", green: "#a6da95", yellow: "#eed49f",
  blue: "#8aadf4", magenta: "#c6a0f6", cyan: "#8bd5ca", white: "#cad3f5",
  brightBlack: "#5b6078", brightRed: "#ff8b92", brightGreen: "#b8f0a5", brightYellow: "#f5dfae",
  brightBlue: "#a5c0ff", brightMagenta: "#d8b9ff", brightCyan: "#a3eee3", brightWhite: "#ffffff",
};

export interface CellSize {
  w: number;
  h: number;
}

let cached: CellSize | null = null;

/** Exact cell size xterm will use (its renderer rounds to device pixels). */
export function cellSize(): CellSize {
  if (cached) return cached;
  const el = document.createElement("div");
  el.style.cssText = "position:absolute;left:-10000px;top:0;width:1000px;height:500px;visibility:hidden";
  document.body.appendChild(el);
  const t = new Terminal({ fontFamily: FONT_FAMILY, fontSize: FONT_SIZE, lineHeight: LINE_HEIGHT, cols: 80, rows: 24 });
  t.open(el);
  const dims = (t as any)._core?._renderService?.dimensions?.css?.cell;
  let size: CellSize;
  if (dims && dims.width > 0 && dims.height > 0) {
    size = { w: dims.width, h: dims.height };
  } else {
    const screen = el.querySelector(".xterm-screen") as HTMLElement | null;
    size = screen ? { w: screen.clientWidth / 80, h: screen.clientHeight / 24 } : { w: 8, h: 17 };
  }
  t.dispose();
  el.remove();
  cached = size;
  return size;
}

export function invalidateCellSize() {
  cached = null;
}

export interface TermHandlers {
  onData: (data: string) => void;
  onBinary: (data: string) => void;
  onFocus?: () => void;
  /** Return true when the app handled the key (terminal must ignore it). */
  onKey?: (e: KeyboardEvent) => boolean;
}

export class ManagedTerminal {
  readonly term: Terminal;
  readonly el: HTMLDivElement;
  readonly search: SearchAddon;
  private opened = false;
  handlers: TermHandlers | null = null;

  constructor(cols: number, rows: number) {
    this.el = document.createElement("div");
    this.el.className = "term-host";
    this.term = new Terminal({
      cols: Math.max(cols, 1),
      rows: Math.max(rows, 1),
      fontFamily: FONT_FAMILY,
      fontSize: FONT_SIZE,
      lineHeight: LINE_HEIGHT,
      theme: THEME,
      scrollback: 10000,
      allowProposedApi: true,
      cursorBlink: true,
      macOptionIsMeta: true,
      rightClickSelectsWord: false,
      // tmux (not us) sends the bell/title; keep xterm simple.
    });
    this.term.loadAddon(new Unicode11Addon());
    this.term.unicode.activeVersion = "11";
    this.term.loadAddon(new WebLinksAddon());
    this.search = new SearchAddon();
    this.term.loadAddon(this.search);
    this.term.onData((d) => this.handlers?.onData(d));
    this.term.onBinary((d) => this.handlers?.onBinary(d));
    this.term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      return !(this.handlers?.onKey?.(e) ?? false);
    });
  }

  /** Mount (or move) into a container. */
  mount(container: HTMLElement) {
    if (this.el.parentElement !== container) container.appendChild(this.el);
    if (!this.opened) {
      this.term.open(this.el);
      this.opened = true;
      this.term.textarea?.addEventListener("focus", () => this.handlers?.onFocus?.());
    }
  }

  resize(cols: number, rows: number) {
    cols = Math.max(cols, 1);
    rows = Math.max(rows, 1);
    if (this.term.cols !== cols || this.term.rows !== rows) this.term.resize(cols, rows);
  }

  /** Replace everything with a snapshot from tmux. */
  reset(cols: number, rows: number, data: Uint8Array) {
    this.term.reset();
    this.term.clear();
    this.resize(cols, rows);
    this.term.write(data);
  }

  write(data: Uint8Array) {
    this.term.write(data);
  }

  focus() {
    this.term.focus();
  }

  dispose() {
    this.term.dispose();
    this.el.remove();
  }
}

const registry = new Map<string, ManagedTerminal>();

export function paneKey(att: number, pane: string) {
  return `a${att}:${pane}`;
}

export function shellKey(shell: number) {
  return `s${shell}`;
}

export function getTerminal(key: string, cols = 80, rows = 24): ManagedTerminal {
  let t = registry.get(key);
  if (!t) {
    t = new ManagedTerminal(cols, rows);
    registry.set(key, t);
  }
  return t;
}

export function peekTerminal(key: string): ManagedTerminal | undefined {
  return registry.get(key);
}

export function disposeTerminal(key: string) {
  registry.get(key)?.dispose();
  registry.delete(key);
}

export function disposeWhere(pred: (key: string) => boolean) {
  for (const k of [...registry.keys()]) if (pred(k)) disposeTerminal(k);
}
