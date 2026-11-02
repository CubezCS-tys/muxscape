import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port in dev and no clearing of its console output.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "es2022", sourcemap: true },
});
