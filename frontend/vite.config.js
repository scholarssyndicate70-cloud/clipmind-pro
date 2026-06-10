import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig(({command}) => ({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    // Required for Tauri to receive HMR events
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  // Tauri 2 uses TAURI_ENV_* env vars; VITE_ for custom app vars
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // Tauri supports es2021 and chrome105+
    target: process.env.TAURI_ENV_PLATFORM === "windows"
      ? "chrome105"
      : ["es2021", "chrome105", "safari13"],
    // Never minify for debug builds
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
}));
