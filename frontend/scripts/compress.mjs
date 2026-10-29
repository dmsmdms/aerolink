// Pre-compresses the build output: writes file.br (brotli q11) and file.gz (gzip -9) next to every
// compressible file, so nginx serves them via brotli_static/gzip_static and spends no CPU on compression.
// A variant is kept only if it saves at least 5 %. Usage: node scripts/compress.mjs <dir>
import { readdirSync, readFileSync, statSync, writeFileSync, unlinkSync } from 'node:fs';
import { join, extname } from 'node:path';
import { brotliCompressSync, gzipSync, constants } from 'node:zlib';

const COMPRESSIBLE = new Set(['.html', '.js', '.mjs', '.css', '.svg', '.json', '.txt', '.xml', '.webmanifest', '.ico', '.wasm']);
const MIN_SIZE = 256;

const root = process.argv[2];
if (!root) {
  console.error('usage: compress.mjs <dir>');
  process.exit(2);
}

function* walk(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(path);
    else yield path;
  }
}

let original = 0, brotli = 0, gzip = 0, files = 0;
for (const path of walk(root)) {
  if (path.endsWith('.br') || path.endsWith('.gz')) {
    unlinkSync(path); // stale variant from an earlier run
    continue;
  }
  if (!COMPRESSIBLE.has(extname(path)) || statSync(path).size < MIN_SIZE) continue;
  const data = readFileSync(path);
  const variants = {
    '.br': brotliCompressSync(data, {
      params: {
        [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY,
        [constants.BROTLI_PARAM_MODE]: extname(path) === '.wasm' ? constants.BROTLI_MODE_GENERIC : constants.BROTLI_MODE_TEXT,
        [constants.BROTLI_PARAM_SIZE_HINT]: data.length,
      },
    }),
    '.gz': gzipSync(data, { level: constants.Z_BEST_COMPRESSION, memLevel: 9 }),
  };
  files++;
  original += data.length;
  for (const [ext, packed] of Object.entries(variants)) {
    const size = packed.length < data.length * 0.95 ? packed.length : data.length;
    if (packed.length < data.length * 0.95) writeFileSync(path + ext, packed);
    if (ext === '.br') brotli += size;
    else gzip += size;
  }
}
const kib = (n) => (n / 1024).toFixed(1) + ' KiB';
console.log(`compressed ${files} files: ${kib(original)} → br ${kib(brotli)}, gz ${kib(gzip)}`);
