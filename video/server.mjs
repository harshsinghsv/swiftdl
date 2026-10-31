// A tiny static server over the repository, so the film's page and the
// app's UI it embeds share one origin and the film can drive them.

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json" };

export function serve() {
  return new Promise((done) => {
    const server = createServer((req, res) => {
      const path = normalize(join(root, decodeURIComponent(new URL(req.url, "http://x").pathname)));
      if (!path.startsWith(root) || !existsSync(path) || statSync(path).isDirectory()) { res.writeHead(404).end(); return; }
      res.writeHead(200, { "Content-Type": TYPES[extname(path)] || "application/octet-stream" });
      createReadStream(path).pipe(res);
    });
    server.listen(0, "127.0.0.1", () => done({ server, origin: `http://127.0.0.1:${server.address().port}` }));
  });
}

export function findBrowser() {
  return [
    process.env.CHROME,
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
  ].filter(Boolean).find(existsSync);
}
