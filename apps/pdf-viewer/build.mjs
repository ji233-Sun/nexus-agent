import { readFile, readdir, mkdir, writeFile } from 'node:fs/promises';
import pako from 'pako';
import { dirname, join } from 'node:path';

// Embed compressed, pinned assets so opening documents never depends on a CDN.
async function copy(source, destination) {
  const bytes = await readFile(source);
  await mkdir(dirname(`dist/${destination}`), { recursive: true });
  await writeFile(`dist/${destination}.gz`, pako.gzip(bytes, { level: 9 }));
}
async function tree(source, destination) {
  for (const entry of await readdir(source, { withFileTypes: true })) {
    if (entry.isDirectory()) await tree(join(source, entry.name), join(destination, entry.name));
    else if (!entry.name.endsWith('.map') && !entry.name.endsWith('.test.mjs')) await copy(join(source, entry.name), join(destination, entry.name));
  }
}
await tree('src', '');
for (const name of ['pdf.mjs', 'pdf.worker.mjs']) await copy(`node_modules/pdfjs-dist/legacy/build/${name}`, `vendor/${name}`);
for (const name of ['pdf_viewer.mjs', 'pdf_viewer.css']) await copy(`node_modules/pdfjs-dist/legacy/web/${name}`, `vendor/${name}`);
for (const name of ['cmaps', 'standard_fonts', 'wasm', 'iccs']) await tree(`node_modules/pdfjs-dist/${name}`, `vendor/${name}`);
await tree('node_modules/pdfjs-dist/web/images', 'vendor/images');
await copy('node_modules/pdf-lib/dist/pdf-lib.esm.min.js', 'vendor/pdf-lib.mjs');
await copy('node_modules/pdfjs-dist/LICENSE', 'vendor/PDFJS-LICENSE');
await copy('node_modules/pdf-lib/LICENSE.md', 'vendor/PDFLIB-LICENSE');
