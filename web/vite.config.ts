import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In dev, run `cargo run -- serve --no-open` and proxy API calls to it.
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: { "/api": "http://127.0.0.1:7878" },
  },
});
