// Finding clickable links in terminal text: normal http(s) URLs, and the
// bare local addresses dev servers and agents print ("localhost:9004/plan").

export interface FoundLink {
  /** Offsets into the scanned text, end exclusive. */
  start: number;
  end: number;
  /** What to open (bare addresses get http://). */
  url: string;
}

const LOCAL = String.raw`(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\])`;
const PATH = String.raw`[^\s<>"'\`]*`;
const LINK_RE = new RegExp(String.raw`\bhttps?://[^\s<>"'\`]+|(?<![\w./:@-])${LOCAL}:\d{1,5}(?:/${PATH})?`, "gi");

/** Drop trailing punctuation that belongs to the sentence, and an unbalanced
 * closing bracket ("(see localhost:3000)"). */
function trimEnd(s: string): string {
  let t = s;
  for (;;) {
    const before = t;
    t = t.replace(/[.,;:!?*]+$/, "");
    for (const [open, close] of [["(", ")"], ["[", "]"], ["{", "}"]]) {
      if (t.endsWith(close) && t.split(open).length < t.split(close).length) t = t.slice(0, -1);
    }
    if (t === before) return t;
  }
}

export function findLinks(text: string): FoundLink[] {
  const out: FoundLink[] = [];
  for (const m of text.matchAll(LINK_RE)) {
    const raw = trimEnd(m[0]);
    if (!raw) continue;
    const isUrl = /^https?:\/\//i.test(raw);
    if (!isUrl) {
      const port = Number(raw.split(":").pop()!.split("/")[0]);
      if (!(port >= 1 && port <= 65535)) continue;
      // Cut short for display ("localhost:51…"): the number isn't the port.
      const after = text.slice(m.index! + m[0].length, m.index! + m[0].length + 3);
      if (after.startsWith("…") || after === "...") continue;
    }
    const start = m.index!;
    out.push({ start, end: start + raw.length, url: isUrl ? raw : `http://${raw}` });
  }
  return out;
}

/** For a local URL: the port it needs forwarded (null for other sites). */
export function localPort(url: string): number | null {
  try {
    const u = new URL(url);
    const local = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"].includes(u.hostname);
    return local && u.port ? Number(u.port) : null;
  } catch {
    return null;
  }
}
