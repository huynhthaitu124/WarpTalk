#!/usr/bin/env node
// Tiny static server for the Next export (`out/`), mirroring the macOS bundle server rules
// (apps/macos/Sources/Web/BundledWeb.swift): exact file -> directory index.html -> `<path>.html` ->
// unknown extensionless path = root index.html (client-side routes) -> else 404.
// Optional: API_PROXY_TARGET=http://127.0.0.1:8080 forwards `/api/**` to the backend (like `next dev` rewrites).
// Usage: node scripts/serve-out.mjs [--port 3000] [--dir out]   (also PORT / OUT_DIR env)
import { createServer, request as httpRequest } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const arg = (n, d) => { const i = process.argv.indexOf(`--${n}`); return i > 0 ? process.argv[i + 1] : d; };
const root = resolve(arg('dir', process.env.OUT_DIR ?? fileURLToPath(new URL('../out', import.meta.url))));
const port = Number(arg('port', process.env.PORT ?? 3000));
const MIME = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.json': 'application/json', '.png': 'image/png', '.svg': 'image/svg+xml', '.ico': 'image/x-icon', '.woff2': 'font/woff2', '.txt': 'text/plain; charset=utf-8', '.map': 'application/json' };

const isFile = async (p) => { try { return (await stat(p)).isFile(); } catch { return false; } };

async function resolveFile(pathname) {
  const rel = normalize(decodeURIComponent(pathname)).replace(/^([/\\])+/, '');
  const abs = join(root, rel);
  if (abs !== root && !abs.startsWith(root + sep)) return null;
  if (await isFile(abs)) return abs;
  if (await isFile(join(abs, 'index.html'))) return join(abs, 'index.html');
  if (!extname(rel)) {
    if (await isFile(`${abs}.html`)) return `${abs}.html`;
    return join(root, 'index.html');
  }
  return null;
}

const proxy = process.env.API_PROXY_TARGET ? new URL(process.env.API_PROXY_TARGET) : null;

createServer(async (req, res) => {
  if (proxy && req.url?.startsWith('/api/')) {
    const up = httpRequest({ host: proxy.hostname, port: proxy.port, path: req.url, method: req.method, headers: { ...req.headers, host: proxy.host } }, (r) => {
      res.writeHead(r.statusCode ?? 502, r.headers);
      r.pipe(res);
    });
    up.on('error', () => res.headersSent || res.writeHead(502).end('bad gateway'));
    req.pipe(up);
    return;
  }
  try {
    const file = await resolveFile(new URL(req.url ?? '/', 'http://x').pathname);
    if (!file || !(await isFile(file))) { res.writeHead(404, { 'content-type': 'text/plain' }).end('not found'); return; }
    res.writeHead(200, { 'content-type': MIME[extname(file)] ?? 'application/octet-stream' }).end(await readFile(file));
  } catch {
    res.writeHead(400).end('bad request');
  }
}).listen(port, '127.0.0.1', () => console.log(`serving ${root} on http://127.0.0.1:${port}`));
