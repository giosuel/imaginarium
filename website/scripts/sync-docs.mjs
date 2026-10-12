// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Copies the repository's developer docs (docs/PLAN.md, docs/design/*,
// docs/adr/*) into the site, so `docs/` stays the single source of truth.
//
// For each file it:
// - takes the first `# Heading` as the page title (Starlight frontmatter)
//   and removes it from the body,
// - rewrites relative links: synced Markdown files become site routes, and
//   everything else points to the file on GitHub,
// - sets `editUrl` to the original file, so "Edit page" edits the source.
//
// Output: src/content/docs/developers/reference/ (git-ignored). It is
// regenerated before every `dev`/`build`.

import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, posix, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = 'https://github.com/giosuel/imaginarium';
const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..', '..');
const outRoot = resolve(here, '..', 'src', 'content', 'docs', 'developers', 'reference');

/** Repo-relative source path → site slug (without base, trailing slash). */
const sources = new Map();
sources.set('docs/PLAN.md', 'developers/reference/plan');
for (const file of readdirSync(join(repoRoot, 'docs', 'design'))) {
  if (file.endsWith('.md') && file !== 'README.md') {
    sources.set(`docs/design/${file}`, `developers/reference/design/${file.replace(/\.md$/, '')}`);
  }
}
for (const file of readdirSync(join(repoRoot, 'docs', 'adr'))) {
  if (/^\d{4}-.+\.md$/.test(file) && !file.startsWith('0000')) {
    sources.set(`docs/adr/${file}`, `developers/reference/adr/${file.replace(/\.md$/, '')}`);
  }
}

/** Rewrites `](target)` links found in `body` of the file at `source`. */
function rewriteLinks(body, source) {
  return body.replace(/\]\(([^)\s]+)\)/g, (match, target) => {
    if (/^([a-z]+:|#)/i.test(target)) return match; // absolute URL or in-page anchor
    const [path, anchor] = target.split('#');
    const resolved = posix.normalize(posix.join(posix.dirname(source), path));
    const suffix = anchor ? `#${anchor}` : '';
    // Root-relative: plugins/remark-base-links.mjs adds the base path.
    if (sources.has(resolved)) return `](/${sources.get(resolved)}/${suffix})`;
    const kind = resolved.endsWith('/') || !posix.extname(resolved) ? 'tree' : 'blob';
    return `](${REPO}/${kind}/main/${resolved.replace(/\/$/, '')}${suffix})`;
  });
}

rmSync(outRoot, { recursive: true, force: true });
for (const [source, slug] of sources) {
  const text = readFileSync(join(repoRoot, source), 'utf8');
  const heading = text.match(/^# (.+)$/m);
  const title = heading ? heading[1].trim() : posix.basename(source, '.md');
  const body = heading ? text.replace(heading[0], '').trimStart() : text;
  const frontmatter = [
    '---',
    `title: ${JSON.stringify(title)}`,
    `editUrl: ${JSON.stringify(`${REPO}/edit/main/${source}`)}`,
    '---',
    '',
    `:::note[Synced from the repository]`,
    `This page is generated from [\`${source}\`](${REPO}/blob/main/${source}).`,
    ':::',
    '',
  ].join('\n');
  const out = join(outRoot, `${relative('developers/reference', slug)}.md`);
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, frontmatter + rewriteLinks(body, source));
}
console.log(`sync-docs: ${sources.size} pages from docs/ → ${relative(repoRoot, outRoot)}`);
