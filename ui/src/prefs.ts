// Appearance preferences: theme, accent colour, terminal text size.
// Per-computer conveniences, so they live in localStorage (host colours are
// saved with the hosts instead).

import { setTerminalAppearance } from "./terminals";

export type ThemeId = "dusk" | "midnight" | "mocha" | "daylight";

export interface ThemeInfo {
  id: ThemeId;
  name: string;
  blurb: string;
  light: boolean;
  /** Default accent when the user hasn't picked one. */
  accent: string;
  /** Preview: [sidebar, work area, raised]. */
  preview: [string, string, string];
}

export const THEMES: ThemeInfo[] = [
  { id: "dusk", name: "Dusk", blurb: "Soft slate, easy on the eyes", light: false, accent: "#9db4ff", preview: ["#181b23", "#1e222b", "#282e3a"] },
  { id: "midnight", name: "Midnight", blurb: "The original: crisp, high contrast", light: false, accent: "#8ab4ff", preview: ["#141820", "#0e1116", "#1a1f29"] },
  { id: "mocha", name: "Mocha", blurb: "Warm charcoal for late nights", light: false, accent: "#e2b07a", preview: ["#1d1a18", "#24201d", "#312b27"] },
  { id: "daylight", name: "Daylight", blurb: "Light, for bright rooms", light: true, accent: "#4466d6", preview: ["#eceef2", "#f7f8fa", "#ffffff"] },
];

/** Accent swatches: a light shade for dark themes, a deeper one for light. */
export const ACCENTS = [
  { id: "periwinkle", name: "Periwinkle", dark: "#9db4ff", light: "#4466d6" },
  { id: "violet", name: "Violet", dark: "#b79cf5", light: "#7a4fd6" },
  { id: "teal", name: "Teal", dark: "#62c3b8", light: "#1f8a7c" },
  { id: "green", name: "Green", dark: "#9fd49a", light: "#3a8443" },
  { id: "amber", name: "Amber", dark: "#e6b46a", light: "#a8700f" },
  { id: "rose", name: "Rose", dark: "#ec8fb0", light: "#c2457a" },
] as const;

export interface Prefs {
  theme: ThemeId;
  /** An ACCENTS id, or null for the theme's own accent. */
  accent: string | null;
  /** Terminal text size in px. */
  fontSize: number;
}

export const DEFAULT_PREFS: Prefs = { theme: "dusk", accent: null, fontSize: 13 };
export const FONT_SIZES = { min: 11, max: 18 };

const KEY = "muxscape.prefs.v1";

export function loadPrefs(): Prefs {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (!raw || typeof raw !== "object") return { ...DEFAULT_PREFS };
    return {
      theme: THEMES.some((t) => t.id === raw.theme) ? raw.theme : DEFAULT_PREFS.theme,
      accent: ACCENTS.some((a) => a.id === raw.accent) ? raw.accent : null,
      fontSize:
        Number.isFinite(raw.fontSize) && raw.fontSize >= FONT_SIZES.min && raw.fontSize <= FONT_SIZES.max
          ? Math.round(raw.fontSize)
          : DEFAULT_PREFS.fontSize,
    };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export function savePrefs(p: Prefs) {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    /* storage unavailable: prefs last for this session only */
  }
}

export function themeInfo(id: ThemeId): ThemeInfo {
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}

/** The accent colour actually used for these prefs. */
export function accentColor(p: Prefs): string {
  const t = themeInfo(p.theme);
  const a = ACCENTS.find((x) => x.id === p.accent);
  return a ? (t.light ? a.light : a.dark) : t.accent;
}

/** Apply to the page (CSS tokens) and to every terminal. */
export function applyPrefs(p: Prefs) {
  const root = document.documentElement;
  root.dataset.theme = p.theme;
  const accent = accentColor(p);
  root.style.setProperty("--accent", accent);
  root.style.colorScheme = themeInfo(p.theme).light ? "light" : "dark";
  setTerminalAppearance(p.theme, accent, p.fontSize);
}
