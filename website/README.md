# Imaginarium website

The public site: landing page, user guides, self-hosting, and developer
docs. It is built with [Astro](https://astro.build) and
[Starlight](https://starlight.astro.build).

```sh
cd website
npm ci            # exact versions from package-lock.json, no install scripts (.npmrc)
npm run dev       # http://localhost:4321/imaginarium/
npm run build     # static site in dist/
npm run check-links   # broken internal links in dist/ (CI runs this)
```

- `src/content/docs/`: hand-written pages (Markdown/MDX).
- `src/content/docs/developers/reference/`: **generated**, git-ignored.
  `scripts/sync-docs.mjs` copies `docs/PLAN.md`, `docs/design/*` and
  `docs/adr/*` here before every `dev`/`build`. Edit the originals in
  `docs/`.
- `src/components/`: Astro components, e.g. the layer diagram (inline SVG
  using Starlight's theme colors).
- `site.config.mjs`: **the only place that knows the URL and base path.**
  Write internal links root-relative (`/developers/architecture/`).
  `plugins/remark-base-links.mjs` adds the base at build time. For a
  custom domain, set `BASE = '/'`. Frontmatter links (e.g. hero buttons)
  don't go through Markdown, so they are written relative to the page.

Dependency policy: exact versions, and new versions only once they have been
public for at least two weeks (`npm install --before=<date>`). Install
scripts are disabled.
