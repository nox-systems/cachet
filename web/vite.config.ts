import stylex from "@stylexjs/unplugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The console is served from the worker's own deployment under /console
// (ADR 0014), so every emitted URL carries that prefix and the alchemy
// program mounts the build at the same base.
export default defineConfig({
  base: "/console/",
  build: { outDir: "dist", emptyOutDir: true },
  plugins: [
    // why: StyleX compiles and aggregates before the React plugin. Its
    // docs require that order to keep Fast Refresh working, and the
    // layer order below puts the atoms above every plain-CSS layer the
    // global entry declares.
    stylex({
      dev: process.env.NODE_ENV === "development",
      test: process.env.NODE_ENV === "test",
      runtimeInjection: false,
      treeshakeCompensation: true,
      unstable_moduleResolution: { type: "commonJS" },
      useCSSLayers: { prefix: "stylex", before: ["theme", "base"] },
    }),
    react(),
  ],
  server: {
    // Local development runs the console against a real worker: `just
    // workerd` or a deployment. Everything the console reads is under
    // these three prefixes.
    proxy: {
      "/api": "http://localhost:8787",
      "/roots": "http://localhost:8787",
      "/_auth": "http://localhost:8787",
    },
  },
});
