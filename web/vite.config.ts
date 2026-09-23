import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In dev, run `cargo run -- serve --no-open` and proxy API calls to it.
// To share the dev server on your tailnet, run `tailscale serve --bg 5173`.
export default defineConfig({
  plugins: [react()],
  server: {
    // `tailscale serve 5173` connects to 127.0.0.1. Vite would otherwise listen on ::1 only.
    host: "127.0.0.1",
    // Vite rejects unknown hostnames. The leading dot allows every Tailscale name, such as
    // joels-mac.tail1234.ts.net. The server still listens only on localhost.
    allowedHosts: [".ts.net"],
    proxy: {
      // changeOrigin sends Host: 127.0.0.1:7878 to the Rust server, which only answers
      // loopback hostnames unless it runs with --allow-host.
      "/api": { target: "http://127.0.0.1:7878", changeOrigin: true },
    },
  },
});
