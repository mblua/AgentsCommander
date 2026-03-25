import { defineConfig } from "vite";
import solidPlugin from "vite-plugin-solid";
import tauriConf from "./src-tauri/tauri.conf.json";

export default defineConfig({
  plugins: [solidPlugin()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  define: {
    __APP_VERSION__: JSON.stringify(tauriConf.version),
  },
  build: {
    target: "esnext",
    minify: !process.env.TAURI_DEBUG,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});
