// Pure geometry over tmux layout trees (cell units).

import type { LayoutCell } from "./types";

export interface Leaf {
  pane: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Divider {
  /** Child indices from the root to the container. */
  path: number[];
  /** Divider sits between children[index] and children[index + 1]. */
  index: number;
  /** "col": a vertical bar between side-by-side panes (drag left/right). */
  orientation: "col" | "row";
  x: number;
  y: number;
  width: number;
  height: number;
}

export function leaves(cell: LayoutCell, out: Leaf[] = []): Leaf[] {
  if (cell.type === "pane") {
    out.push({ pane: cell.value as string, x: cell.x, y: cell.y, width: cell.width, height: cell.height });
  } else {
    for (const c of cell.value as LayoutCell[]) leaves(c, out);
  }
  return out;
}

export function dividers(cell: LayoutCell, path: number[] = [], out: Divider[] = []): Divider[] {
  if (cell.type === "pane") return out;
  const kids = cell.value as LayoutCell[];
  kids.forEach((c, i) => {
    dividers(c, [...path, i], out);
    if (i === kids.length - 1) return;
    if (cell.type === "horizontal") {
      out.push({ path, index: i, orientation: "col", x: c.x + c.width, y: cell.y, width: 1, height: cell.height });
    } else {
      out.push({ path, index: i, orientation: "row", x: cell.x, y: c.y + c.height, width: cell.width, height: 1 });
    }
  });
  return out;
}

export type Direction = "left" | "right" | "up" | "down";

/** Nearest pane in a direction, preferring the one overlapping the most. */
export function neighbor(all: Leaf[], from: string, dir: Direction): string | null {
  const a = all.find((l) => l.pane === from);
  if (!a) return null;
  let best: { pane: string; dist: number; overlap: number } | null = null;
  for (const b of all) {
    if (b.pane === from) continue;
    let dist: number, overlap: number;
    if (dir === "left" || dir === "right") {
      dist = dir === "left" ? a.x - (b.x + b.width) : b.x - (a.x + a.width);
      overlap = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y);
    } else {
      dist = dir === "up" ? a.y - (b.y + b.height) : b.y - (a.y + a.height);
      overlap = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x);
    }
    if (dist < 0 || overlap <= 0) continue;
    if (!best || dist < best.dist || (dist === best.dist && overlap > best.overlap)) {
      best = { pane: b.pane, dist, overlap };
    }
  }
  return best?.pane ?? null;
}

/** Simple fuzzy match score (subsequence, bonus for contiguous / prefix). */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  if (!q) return 0;
  const direct = t.indexOf(q);
  if (direct >= 0) return 1000 - direct * 2 - t.length * 0.01;
  let ti = 0;
  let score = 0;
  let streak = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    streak = found === ti ? streak + 1 : 0;
    score += 10 + streak * 5 - (found - ti);
    ti = found + 1;
  }
  return score;
}
