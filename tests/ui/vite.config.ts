import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// Separate verification output; this entry is never part of the app bundle.
export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "test-results/built-ui",
    rollupOptions: { input: "tests/ui/index.html" },
  },
});
