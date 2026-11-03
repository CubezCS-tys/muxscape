// Persistent xterm.js instances, one per remote pane (and per plain shell).
//
// Terminals live outside React: switching tabs re-parents the existing DOM
// node instead of recreating anything, and pane output is written straight
// to the emulator without touching React state.

import { Terminal, type ITheme } from "@xterm/xterm";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import type { ILink, ILinkProvider } from "@xterm/xterm";
import { findLinks, localPort } from "./links";
import { SearchAddon } from "@xterm/addon-search";

export const FONT_FAMILY =
  '"JetBrains Mono", "Cascadia Mono", "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", "Liberation Mono", monospace';
export const FONT_SIZE = 13;
export const LINE_HEIGHT = 1.2;

/** Terminal colours per app theme. The background always matches the work
 * area so the terminal never looks like a separate slab. */
const TERM_THEMES: Record<string, ITheme> = {
  // Soft slate, lower-saturation palette for long reading sessions.
  dusk: {
    background: "#1e222b", foreground: "#cdd3de", selectionBackground: "#3a4566",
    black: "#2a2f3a", red: "#e88388", green: "#a6d39e", yellow: "#e3c78a",
    blue: "#8fb0f0", magenta: "#c5a3ec", cyan: "#88cfc5", white: "#c4cad6",
    brightBlack: "#6b7385", brightRed: "#f09ca0", brightGreen: "#bfe3b6", brightYellow: "#edd6a3",
    brightBlue: "#a9c3f7", brightMagenta: "#d6bdf5", brightCyan: "#a6e0d8", brightWhite: "#eef1f6",
  },
  // The original: near-black, crisp and high-contrast.
  midnight: {
    background: "#0e1116", foreground: "#d6dbe4", selectionBackground: "#3a4a6b",
    black: "#1b1f27", red: "#f07178", green: "#a6da95", yellow: "#eed49f",
    blue: "#8aadf4", magenta: "#c6a0f6", cyan: "#8bd5ca", white: "#cad3f5",
    brightBlack: "#5b6078", brightRed: "#ff8b92", brightGreen: "#b8f0a5", brightYellow: "#f5dfae",
    brightBlue: "#a5c0ff", brightMagenta: "#d8b9ff", brightCyan: "#a3eee3", brightWhite: "#ffffff",
  },
  // Warm charcoal, easiest late at night.
  mocha: {
    background: "#24201d", foreground: "#ddd3c8", selectionBackground: "#4a3f36",
    black: "#332d29", red: "#e58f84", green: "#b5c98e", yellow: "#e6c27e",
    blue: "#8fb2d6", magenta: "#cfa0c3", cyan: "#8fc9b8", white: "#d0c6ba",
    brightBlack: "#7a6e64", brightRed: "#f0a89e", brightGreen: "#c9dba6", brightYellow: "#f0d49a",
    brightBlue: "#a9c6e4", brightMagenta: "#dfb7d4", brightCyan: "#a9dccc", brightWhite: "#f4ece2",
  },
  // Light: darker ANSI colours so they read on a pale background.
  daylight: {
    background: "#f7f8fa", foreground: "#2a2f3a", selectionBackground: "#c9d6f5",
    black: "#2a2f3a", red: "#c4424b", green: "#3a8443", yellow: "#946a0e",
    blue: "#3b63c9", magenta: "#8a4fc4", cyan: "#23838a", white: "#7a8292",
    brightBlack: "#5f6878", brightRed: "#d4565e", brightGreen: "#4a9a53", brightYellow: "#ab7d16",
    brightBlue: "#4f76d8", brightMagenta: "#9b62d2", brightCyan: "#2f9aa1", brightWhite: "#11151c",
  },
};

/** What new and existing terminals look like (set from the user's prefs). */
const appearance = { theme: "dusk", cursor: "#9db4ff", fontSize: FONT_SIZE };

function currentTheme(): ITheme {
  const t = TERM_THEMES[appearance.theme] ?? TERM_THEMES.dusk;
  return { ...t, cursor: appearance.cursor, cursorAccent: t.background };
}

/** Apply a theme / cursor colour / text size to every terminal, now and later. */
export function setTerminalAppearance(theme: string, cursor: string, fontSize: number) {
  const sizeChanged = fontSize !== appearance.fontSize;
  Object.assign(appearance, { theme, cursor, fontSize });
  if (sizeChanged) invalidateCellSize();
  for (const t of registry.values()) {
    t.term.options.theme = currentTheme();
    if (sizeChanged) t.term.options.fontSize = fontSize;
  }
}

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
  const t = new Terminal({ fontFamily: FONT_FAMILY, fontSize: appearance.fontSize, lineHeight: LINE_HEIGHT, cols: 80, rows: 24 });
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

/** Link clicks: (terminal key, url). Set by the app (forwards localhost links). */
let linkHandler: ((key: string, uri: string) => void) | null = null;
export function setLinkHandler(h: (key: string, uri: string) => void) {
  linkHandler = h;
}

const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);

// One small tooltip shared by all terminals: "Ctrl+click to open".
let tip: HTMLDivElement | null = null;
function showLinkTip(e: MouseEvent, url: string) {
  if (!tip) {
    tip = document.createElement("div");
    tip.className = "link-tip";
    document.body.appendChild(tip);
  }
  const port = localPort(url);
  tip.textContent = `${isMac ? "⌘" : "Ctrl"}+click to open${port ? ` · forwards port ${port}` : ""}`;
  tip.style.left = `${e.clientX + 12}px`;
  tip.style.top = `${e.clientY + 16}px`;
  tip.style.display = "block";
}
function hideLinkTip() {
  if (tip) tip.style.display = "none";
}

/** Links in a terminal (URLs and bare "localhost:9004/plan"), opened with
 * Ctrl/⌘+click so selecting text never opens anything by accident. Lines
 * the terminal wrapped are joined so a long link works across rows. */
function linkProvider(term: Terminal, key: string): ILinkProvider {
  return {
    provideLinks(y, callback) {
      const buf = term.buffer.active;
      const cols = term.cols;
      let first = y - 1;
      while (first > 0 && buf.getLine(first)?.isWrapped) first--;
      let last = first;
      while (buf.getLine(last + 1)?.isWrapped) last++;
      let text = "";
      for (let r = first; r <= last; r++) text += buf.getLine(r)?.translateToString(r === last) ?? "";
      const links: ILink[] = [];
      for (const l of findLinks(text)) {
        const start = { x: (l.start % cols) + 1, y: first + Math.floor(l.start / cols) + 1 };
        const end = { x: ((l.end - 1) % cols) + 1, y: first + Math.floor((l.end - 1) / cols) + 1 };
        if (y < start.y || y > end.y) continue;
        links.push({
          range: { start, end },
          text: l.url,
          decorations: { underline: true, pointerCursor: true },
          activate: (e, url) => {
            if (!(e.ctrlKey || e.metaKey)) return;
            e.preventDefault();
            hideLinkTip();
            if (linkHandler) linkHandler(key, url);
            else window.open(url, "_blank", "noopener");
          },
          hover: (e, url) => showLinkTip(e, url),
          leave: hideLinkTip,
        });
      }
      callback(links.length ? links : undefined);
    },
  };
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

  constructor(cols: number, rows: number, key = "") {
    this.el = document.createElement("div");
    this.el.className = "term-host";
    this.term = new Terminal({
      cols: Math.max(cols, 1),
      rows: Math.max(rows, 1),
      fontFamily: FONT_FAMILY,
      fontSize: appearance.fontSize,
      lineHeight: LINE_HEIGHT,
      theme: currentTheme(),
      scrollback: 10000,
      allowProposedApi: true,
      cursorBlink: true,
      macOptionIsMeta: true,
      rightClickSelectsWord: false,
      // tmux (not us) sends the bell/title; keep xterm simple.
    });
    this.term.loadAddon(new Unicode11Addon());
    this.term.unicode.activeVersion = "11";
    this.term.registerLinkProvider(linkProvider(this.term, key));
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
    t = new ManagedTerminal(cols, rows, key);
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

/** The last non-empty lines of a terminal (for drag previews). */
export function previewLines(key: string, n = 8): string[] {
  const t = registry.get(key);
  if (!t) return [];
  const buf = t.term.buffer.active;
  const out: string[] = [];
  for (let y = buf.baseY + buf.cursorY; y >= 0 && out.length < n; y--) {
    const line = buf.getLine(y)?.translateToString(true) ?? "";
    if (line.trim() || out.length) out.unshift(line);
  }
  while (out.length && !out[out.length - 1].trim()) out.pop();
  return out.slice(-n);
}
