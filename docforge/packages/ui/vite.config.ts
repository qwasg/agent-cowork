import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/**
 * 让 Vite 能解析 monorepo 内部源码包里 ESM 风格的 ".js" 导入(实际为 .ts)。
 * doc-core 等包使用 `./foo.js` 写法以兼容 tsc/node;Vite 默认不会回退到 .ts。
 */
function resolveTsFromJs(): Plugin {
  return {
    name: "docforge-resolve-ts-from-js",
    enforce: "pre",
    async resolveId(source, importer) {
      if (!importer || !source.startsWith(".") || !source.endsWith(".js")) return null;
      if (!importer.includes("packages")) return null;
      const tsPath = resolve(dirname(importer), source.slice(0, -3) + ".ts");
      if (existsSync(tsPath)) return tsPath;
      return null;
    },
  };
}

const root = dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  root,
  plugins: [resolveTsFromJs(), react()],
  server: {
    port: 5173,
    strictPort: false,
  },
  resolve: {
    dedupe: ["yjs", "react", "react-dom", "y-prosemirror"],
  },
  optimizeDeps: {
    exclude: ["@docforge/doc-core"],
  },
});
