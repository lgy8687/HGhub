import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const abs = (p: string) => fileURLToPath(new URL(p, import.meta.url));
const { version } = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
) as { version: string };

/**
 * 网页版构建：与原生构建共用同一份 src，只做两件事——
 * 1. 把 @tauri-apps/api/{core,window} alias 到浏览器 shim；
 * 2. 用 __WEB__ 让 src/api.ts 的媒体基址从 hg:// 换成 /hg。
 * 产物直接输出到 hgweb/web/，由 axum 服务托管。
 */
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  define: {
    __APP_VERSION__: JSON.stringify(version),
    __WEB__: "true",
  },
  resolve: {
    alias: [
      { find: /^@tauri-apps\/api\/core$/, replacement: abs("./src/web/tauri-core.ts") },
      { find: /^@tauri-apps\/api\/window$/, replacement: abs("./src/web/tauri-window.ts") },
    ],
  },
  build: {
    target: "es2021",
    minify: "esbuild",
    sourcemap: false,
    outDir: "hgweb/web",
    emptyOutDir: true,
    rollupOptions: { input: abs("./web.html") },
  },
});
