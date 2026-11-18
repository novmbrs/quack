import { fileURLToPath } from "node:url";

import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, lazyPlugins } from "vite-plus";

// In production `quack serve` serves the built app and the API from one
// origin. In development Vite proxies /api (and the health endpoints) to it,
// so the browser still talks to one origin and the session cookies just work.
const backend = process.env.QUACK_DASHBOARD_API_URL ?? "http://localhost:8080";
const src = fileURLToPath(new URL("./src", import.meta.url));

export default defineConfig({
  // Unit tests cover plain TypeScript; the build plugins only slow them down
  // and keep watchers open after the run.
  plugins: lazyPlugins(() =>
    process.env.VITEST
      ? []
      : [
          tanstackRouter({
            target: "react",
            autoCodeSplitting: true,
            generatedRouteTree: "src/routeTree.gen.ts",
            quoteStyle: "double",
          }),
          react(),
        ],
  ),
  resolve: {
    alias: { "~": src },
  },
  server: {
    port: 3000,
    strictPort: true,
    // The icon set lives in the repo's shared assets directory.
    fs: { allow: [fileURLToPath(new URL("../..", import.meta.url))] },
    proxy: {
      // Keep the browser's Host so the backend sees the dashboard origin in
      // redirects, cookies, and the Origin check on writes.
      "/api": { target: backend, changeOrigin: false },
      "/livez": { target: backend, changeOrigin: false },
      "/readyz": { target: backend, changeOrigin: false },
    },
  },
  build: {
    outDir: "dist/app",
    // Icons ship as separate cached files, so a page only loads the ones it shows.
    assetsInlineLimit: (file) => (file.endsWith(".svg") ? false : undefined),
    target: "es2023",
    sourcemap: true,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
  fmt: {
    ignorePatterns: ["src/routeTree.gen.ts", "src/api/schema.gen.ts"],
  },
  lint: {
    plugins: ["react", "typescript", "oxc"],
    ignorePatterns: ["src/routeTree.gen.ts", "src/api/schema.gen.ts", "dist/**"],
    rules: {
      "react/rules-of-hooks": "error",
      // Route files export Route next to their components; the router
      // plugin handles their hot updates.
      "react/only-export-components": "off",
      "vite-plus/prefer-vite-plus-imports": "error",
    },
    options: { typeAware: true, typeCheck: true },
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
  },
});
