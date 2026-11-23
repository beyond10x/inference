import {createServer} from 'node:http';
import {readFile, stat} from 'node:fs/promises';
import {extname, join, normalize, resolve} from 'node:path';
import {adapter} from '../adapter.ts';

const root = resolve(import.meta.dirname, '..', '..', 'website', 'build');
const port = Number(process.env.UX_PORT ?? adapter.port);

const types: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.ico': 'image/x-icon',
  '.json': 'application/json',
  '.xml': 'application/xml',
  '.txt': 'text/plain',
  '.woff2': 'font/woff2',
  '.woff': 'font/woff',
  '.webmanifest': 'application/manifest+json',
};

async function isFile(path: string): Promise<boolean> {
  try {
    return (await stat(path)).isFile();
  } catch {
    return false;
  }
}

async function locate(urlPath: string): Promise<string | undefined> {
  if (!urlPath.startsWith(adapter.basePath)) return undefined;
  const rel = normalize(decodeURIComponent(urlPath.slice(adapter.basePath.length)));
  if (rel.startsWith('..')) return undefined;
  const base = join(root, rel);
  for (const candidate of [base, `${base}.html`, join(base, 'index.html')]) {
    if (await isFile(candidate)) return candidate;
  }
  return undefined;
}

createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost');
  const file = await locate(url.pathname);
  if (!file) {
    const notFound = join(root, '404.html');
    res.writeHead(404, {'content-type': types['.html']});
    res.end(await isFile(notFound) ? await readFile(notFound) : 'not found');
    return;
  }
  res.writeHead(200, {'content-type': types[extname(file)] ?? 'application/octet-stream'});
  res.end(await readFile(file));
}).listen(port, '127.0.0.1', () => {
  process.stdout.write(`serving ${root} at http://127.0.0.1:${port}${adapter.basePath}\n`);
});
