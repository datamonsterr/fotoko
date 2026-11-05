// Same-origin front for the simulated stack: serves the exported Expo web app (with a fallback
// to index.html for client-side routes) and proxies the API paths to fotoko-local. The API has
// no CORS headers, and with one origin the browser needs none.
//
// Environment: GATEWAY_PORT, GATEWAY_HOST (default 127.0.0.1), API_URL (e.g. http://127.0.0.1:8787), STATIC_DIR (expo export dir).
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import http from 'node:http';
import path from 'node:path';

const port = Number(process.env.GATEWAY_PORT);
const host = process.env.GATEWAY_HOST ?? '127.0.0.1';
const api = new URL(process.env.API_URL ?? '');
const root = path.resolve(process.env.STATIC_DIR ?? '');
if (!Number.isInteger(port) || port <= 0) {
  throw new Error('GATEWAY_PORT must be a port number');
}

const PROXIED = [/^\/v1\//, /^\/healthz$/, /^\/_objects\//];
const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.ttf': 'font/ttf',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.map': 'application/json',
};

function proxy(req, res) {
  const upstream = http.request(
    {
      host: api.hostname,
      port: api.port,
      method: req.method,
      path: req.url,
      headers: req.headers,
    },
    (reply) => {
      res.writeHead(reply.statusCode ?? 502, reply.headers);
      reply.pipe(res);
    },
  );
  upstream.on('error', (error) => {
    res.writeHead(502, { 'content-type': 'text/plain' });
    res.end(`API unreachable: ${error.message}`);
  });
  req.pipe(upstream);
}

async function isFile(file) {
  try {
    return (await stat(file)).isFile();
  } catch {
    return false;
  }
}

async function serveStatic(req, res) {
  const { pathname } = new URL(req.url ?? '/', 'http://gateway');
  let file = path.join(root, path.normalize(decodeURIComponent(pathname)));
  if (file !== root && !file.startsWith(root + path.sep)) {
    res.writeHead(403).end('forbidden');
    return;
  }
  if (!(await isFile(file))) {
    // A path with an extension is an asset that does not exist; anything else is a route.
    if (path.extname(pathname) !== '') {
      res.writeHead(404).end('not found');
      return;
    }
    file = path.join(root, 'index.html');
  }
  res.writeHead(200, {
    'content-type': TYPES[path.extname(file)] ?? 'application/octet-stream',
    'cache-control': 'no-store',
  });
  createReadStream(file).pipe(res);
}

http
  .createServer((req, res) => {
    const url = req.url ?? '/';
    if (PROXIED.some((pattern) => pattern.test(url))) {
      proxy(req, res);
    } else {
      serveStatic(req, res).catch((error) => {
        res.writeHead(500).end(String(error));
      });
    }
  })
  .listen(port, host, () => {
    console.log(`gateway listening on http://${host}:${port}`);
  });
