import { createRoot } from "react-dom/client";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";
import { App } from "./App";
import { invalidateCellSize } from "./terminals";

// Re-measure cells once web fonts are ready (metrics may change).
document.fonts?.ready.then(() => invalidateCellSize());

createRoot(document.getElementById("root")!).render(<App />);
