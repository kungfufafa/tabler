import path from "node:path";
import process from "node:process";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  define: {
    // "mock" mode runs the renderer in a plain browser with an in-memory IPC transport.
    __TABLER_MOCK__: JSON.stringify(mode === "mock"),
  },
  resolve: {
    alias: { "@": path.resolve(import.meta.dirname, "src") },
    // CodeMirror breaks ("Unrecognized extension value") when two copies of @codemirror/state load.
    dedupe: ["@codemirror/state", "@codemirror/view", "react", "react-dom"],
  },
  optimizeDeps: {
    include: ["@codemirror/state", "@codemirror/view", "@codemirror/lang-sql"],
  },
  // prevent Vite from obscuring rust errors
  clearScreen: false,
  server: {
    // tauri expects a fixed port, fail if that port is not available
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    // tell Vite to ignore watching `src-tauri`
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // WKWebView on macOS 12+ is far newer than this; keep a conservative floor.
    target: "safari16",
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
}));
