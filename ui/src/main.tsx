import { createRoot } from "react-dom/client";
import "@xterm/xterm/css/xterm.css";
// Bundled fonts: the same look on every machine (and offline).
import "@fontsource-variable/manrope";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/700.css";
import "./styles.css";
import { App } from "./App";
import { invalidateCellSize } from "./terminals";
import { applyPrefs, loadPrefs } from "./prefs";

// Theme first, so nothing flashes in the wrong colours.
applyPrefs(loadPrefs());

// Terminal cells are measured once, so the terminal font must be loaded
// before any terminal exists (web fonts otherwise load on first use, after
// xterm has measured a fallback). Bundled, so this takes milliseconds; the
// timeout only guards against a broken font.
const fontsReady = Promise.all(
  ['13px "JetBrains Mono"', 'bold 13px "JetBrains Mono"', '500 13px "Manrope Variable"'].map((f) => document.fonts?.load(f)),
);
Promise.race([fontsReady, new Promise((r) => setTimeout(r, 1500))]).finally(() => {
  invalidateCellSize();
  createRoot(document.getElementById("root")!).render(<App />);
});
