import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1421,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    sourcemap: false,
    outDir: "dist-editor",
    emptyOutDir: true,
    rollupOptions: {
      input: "editor.html",
    },
  },
});
