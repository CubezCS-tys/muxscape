// Host tile colours: soft, distinct hues that read on every theme. A host
// keeps its colour (chosen, or derived from its name) so prod and dev are
// recognisable at a glance.

export const TILE_COLORS = ["#e8907f", "#e0b067", "#a3c47f", "#62c3b8", "#6fb2e8", "#9aa3f2", "#c08fe6", "#9aa1ad"];
const AUTO = TILE_COLORS.slice(0, 7);

export function tileColor(label: string, chosen?: string | null) {
  if (chosen && /^#[0-9a-f]{6}$/i.test(chosen)) return chosen;
  let h = 2166136261;
  for (let i = 0; i < label.length; i++) h = Math.imul(h ^ label.charCodeAt(i), 16777619);
  return AUTO[(h >>> 0) % AUTO.length];
}

export function initials(label: string) {
  const words = label.split(/[\s\-_.@/]+/).filter((w) => /[a-z0-9]/i.test(w));
  const s = words.length >= 2 ? words[0][0] + words[1][0] : (words[0] ?? "?").slice(0, 2);
  return s.toUpperCase();
}
