// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Checks the built site (dist/) for broken internal links: every root-
// relative href must carry the base path exactly once and point to an
// existing page or file. Run after `npm run build`. CI does this.

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { BASE } from '../site.config.mjs';

const base = BASE.replace(/\/+$/, '');
const dist = new URL('../dist/', import.meta.url).pathname;

/** All HTML files below `dir`. */
function htmlFiles(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return htmlFiles(path);
    return name.endsWith('.html') ? [path] : [];
  });
}

const problems = new Set();
let checked = 0;
for (const file of htmlFiles(dist)) {
  const page = file.slice(dist.length - 1);
  for (const [, href] of readFileSync(file, 'utf8').matchAll(/href="(\/[^"#?]*)/g)) {
    if (href.startsWith('//')) continue; // protocol-relative: external
    checked += 1;
    if (base && !(href === base || href.startsWith(`${base}/`))) {
      problems.add(`${page}: ${href} (missing base ${base})`);
      continue;
    }
    if (base && href.startsWith(`${base}${base}/`)) {
      problems.add(`${page}: ${href} (base applied twice)`);
      continue;
    }
    const path = join(dist, href.slice(base.length));
    if (!existsSync(path) || (statSync(path).isDirectory() && !existsSync(join(path, 'index.html')))) {
      problems.add(`${page}: ${href} (no such page)`);
    }
  }
}

if (problems.size > 0) {
  console.error(`check-links: ${problems.size} broken link(s):`);
  for (const problem of [...problems].sort()) console.error(`  - ${problem}`);
  process.exit(1);
}
console.log(`check-links: ok (${checked} internal links)`);
