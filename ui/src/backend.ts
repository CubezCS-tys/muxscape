// Transport to the Rust core: Tauri IPC inside the desktop app, or the
// WebSocket dev bridge (crates/mux-devserver) in a normal browser.

import type { MuxEvent, Request } from "./types";

export interface Backend {
  request<T = unknown>(req: Request): Promise<T>;
  /** Keystrokes and resizes: must arrive in order; fire and forget. */
  send(req: Request): void;
  onEvent(cb: (e: MuxEvent) => void): void;
}

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

class TauriBackend implements Backend {
  private listeners: ((e: MuxEvent) => void)[] = [];
  private ready: Promise<void>;

  constructor() {
    this.ready = import("@tauri-apps/api/event").then(({ listen }) =>
      listen<MuxEvent>("mux-event", (e) => this.listeners.forEach((l) => l(e.payload))).then(() => undefined),
    );
  }

  async request<T>(req: Request): Promise<T> {
    await this.ready;
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("mux_request", { req });
  }

  send(req: Request): void {
    // `mux_send` is a synchronous command: Tauri runs those in order.
    import("@tauri-apps/api/core").then(({ invoke }) => invoke("mux_send", { req })).catch(console.error);
  }

  onEvent(cb: (e: MuxEvent) => void) {
    this.listeners.push(cb);
  }
}

class WsBackend implements Backend {
  private ws!: WebSocket;
  private nextId = 1;
  private pending = new Map<number, { resolve: (v: any) => void; reject: (e: any) => void }>();
  private listeners: ((e: MuxEvent) => void)[] = [];
  private open: Promise<void>;
  private queue: string[] = [];

  constructor() {
    this.open = this.connect();
  }

  private connect(): Promise<void> {
    const token = new URLSearchParams(location.search).get("token") ?? "";
    const proto = location.protocol === "https:" ? "wss" : "ws";
    this.ws = new WebSocket(`${proto}://${location.host}/ws?token=${encodeURIComponent(token)}`);
    this.ws.onmessage = (m) => {
      const msg = JSON.parse(m.data);
      if (msg.event) {
        this.listeners.forEach((l) => l(msg.event));
      } else if (msg.id) {
        const p = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (p) "err" in msg ? p.reject(msg.err) : p.resolve(msg.ok);
      }
    };
    return new Promise((resolve, reject) => {
      this.ws.onopen = () => {
        this.queue.forEach((q) => this.ws.send(q));
        this.queue = [];
        resolve();
      };
      this.ws.onerror = () => reject(new Error("dev bridge connection failed (check the ?token=)"));
    });
  }

  private raw(req: Request): Promise<any> {
    const id = this.nextId++;
    const text = JSON.stringify({ id, req });
    const p = new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
    if (this.ws.readyState === WebSocket.OPEN) this.ws.send(text);
    else this.queue.push(text);
    return p;
  }

  async request<T>(req: Request): Promise<T> {
    await this.open;
    return this.raw(req) as Promise<T>;
  }

  send(req: Request): void {
    this.raw(req).catch((e) => console.warn("send failed", e));
  }

  onEvent(cb: (e: MuxEvent) => void) {
    this.listeners.push(cb);
  }
}

export const backend: Backend = isTauri ? new TauriBackend() : new WsBackend();

// ---- base64 helpers (pane data travels as base64 inside JSON)

export function b64ToBytes(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function bytesToB64(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i += 0x8000) {
    s += String.fromCharCode(...b.subarray(i, i + 0x8000));
  }
  return btoa(s);
}

const enc = new TextEncoder();
export function strToB64(s: string): string {
  return bytesToB64(enc.encode(s));
}

/** xterm's onBinary gives a "binary string" (one char per byte). */
export function binaryStrToB64(s: string): string {
  return btoa(s);
}

// ---- clipboard: Tauri plugin in the app (webview clipboard access varies
// by platform), the async Clipboard API in a browser.

export const clipboard = {
  async read(): Promise<string> {
    try {
      if (isTauri) return await (await import("@tauri-apps/plugin-clipboard-manager")).readText();
      return await navigator.clipboard.readText();
    } catch {
      return "";
    }
  },
  /** An image on the clipboard, as PNG bytes (screenshots), if any. */
  async readImage(): Promise<Uint8Array | null> {
    try {
      if (isTauri) {
        const img = await (await import("@tauri-apps/plugin-clipboard-manager")).readImage();
        const { width, height } = await img.size();
        const rgba = await img.rgba();
        return await rgbaToPng(rgba, width, height);
      }
      for (const item of await navigator.clipboard.read()) {
        const type = item.types.find((t) => t.startsWith("image/"));
        if (type) return new Uint8Array(await (await item.getType(type)).arrayBuffer());
      }
    } catch {
      /* no image, or no permission */
    }
    return null;
  },
  async write(text: string): Promise<void> {
    try {
      if (isTauri) await (await import("@tauri-apps/plugin-clipboard-manager")).writeText(text);
      else await navigator.clipboard.writeText(text);
    } catch {
      /* ignore */
    }
  },
};

// ---- opening URLs in the user's real browser (Tauri opener plugin in the
// app; a new tab in a browser).

/**
 * Call synchronously from a click handler, *before* any await: browsers only
 * allow popups during the click itself. Returns a function that navigates
 * once the URL is known (pass null to cancel).
 */
export function externalOpener(): (url: string | null) => void {
  if (isTauri) {
    return (url) => {
      if (url) import("@tauri-apps/plugin-opener").then(({ openUrl }) => openUrl(url)).catch(console.error);
    };
  }
  const w = window.open("about:blank", "_blank");
  return (url) => {
    if (!w) {
      if (url) window.open(url, "_blank", "noopener");
    } else if (url) {
      w.opener = null;
      w.location.href = url;
    } else {
      w.close();
    }
  };
}

export function openExternal(url: string) {
  externalOpener()(url);
}

/** Close this app window (pop-outs close themselves when their tab leaves). */
export function closeCurrentWindow() {
  if (isTauri) import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().close());
  else window.close();
}

/** Encode raw RGBA pixels as PNG (via a canvas: no extra dependency). */
async function rgbaToPng(rgba: Uint8Array, width: number, height: number): Promise<Uint8Array | null> {
  if (!width || !height) return null;
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), width, height), 0, 0);
  const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, "image/png"));
  return blob ? new Uint8Array(await blob.arrayBuffer()) : null;
}
