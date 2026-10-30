import { defineConfig } from "vite";

// Tauri expects a fixed dev port and no HMR overlay stealing focus in the character window.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: ["es2022", "chrome110", "safari16"], sourcemap: false },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
} as any);
