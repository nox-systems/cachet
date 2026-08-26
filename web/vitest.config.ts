import stylex from "@stylexjs/unplugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// The console lane (docs/testing/console.md). Two kinds of test share
// one runner: pure derivations, which need no DOM, and presentational
// components under Testing Library, which take props and no queries.
export default defineConfig({
  plugins: [
    // why: StyleX runs in test mode too, so a component renders with the
    // class names it will ship with rather than with nothing.
    stylex({
      test: true,
      runtimeInjection: false,
      unstable_moduleResolution: { type: "commonJS" },
    }),
    react(),
  ],
  test: {
    environment: "happy-dom",
    include: ["test/**/*.test.ts", "test/**/*.test.tsx"],
    globals: false,
    restoreMocks: true,
    teardownTimeout: 1_000,
    // why: forks. Under the default worker pool, happy-dom keeps a handle
    // open and the run hangs for ten seconds after every green suite,
    // which turns a fast lane into a slow one for no signal.
    pool: "forks",
  },
});
