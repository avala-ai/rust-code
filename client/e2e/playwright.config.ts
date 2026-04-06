import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: '*.spec.ts',
  timeout: 30_000,
  retries: 1,
  use: {
    // The Flutter WASM build is served by a local HTTP server.
    // Override with WASM_SERVER_URL env var if needed.
    baseURL: process.env.WASM_SERVER_URL || 'http://localhost:9090',
    screenshot: 'only-on-failure',
    trace: 'on-first-retry',
  },
  webServer: {
    // Build and serve the Flutter WASM app before tests.
    // Set SKIP_BUILD=1 to skip the build (use pre-built assets).
    command: process.env.SKIP_BUILD
      ? 'python3 -c "import http.server,socketserver;h=type(\'H\',(http.server.SimpleHTTPRequestHandler,),{\'extensions_map\':{**http.server.SimpleHTTPRequestHandler.extensions_map,\'.wasm\':\'application/wasm\'},\'end_headers\':lambda s:(s.send_header(\'Cross-Origin-Opener-Policy\',\'same-origin\'),s.send_header(\'Cross-Origin-Embedder-Policy\',\'require-corp\'),super(type(s),s).end_headers())});socketserver.TCPServer.allow_reuse_address=True;socketserver.TCPServer((\'0.0.0.0\',9090),h).serve_forever()" '
      : 'cd .. && flutter build web --wasm && cd build/web && python3 -m http.server 9090',
    port: 9090,
    reuseExistingServer: true,
    timeout: 180_000, // WASM build can take 2-3 minutes
  },
  projects: [
    {
      name: 'chromium',
      use: { browserName: 'chromium' },
    },
  ],
});
