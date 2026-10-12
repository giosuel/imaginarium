// SPDX-License-Identifier: AGPL-3.0-or-later
// Imaginarium website (Astro + Starlight). See README.md in this directory.
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  // Updated when the site is deployed (GitHub Pages or a custom domain).
  site: 'https://giosuel.github.io',
  base: '/imaginarium',
  integrations: [
    starlight({
      title: 'Imaginarium',
      description:
        'One fast, open office suite: documents, slides, layouts, diagrams and paint on one board, with built-in AI and end-to-end encrypted collaboration.',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/giosuel/imaginarium' },
      ],
      editLink: {
        baseUrl: 'https://github.com/giosuel/imaginarium/edit/main/website/',
      },
      lastUpdated: false,
      sidebar: [
        { label: 'Download', link: '/download/' },
        { label: 'User guide', items: [{ autogenerate: { directory: 'guide' } }] },
        { label: 'Self-hosting', items: [{ autogenerate: { directory: 'self-hosting' } }] },
        {
          label: 'Developers',
          items: [
            { label: 'Overview', link: '/developers/' },
            { label: 'Architecture', link: '/developers/architecture/' },
            { label: 'Contributing', link: '/developers/contributing/' },
            { label: 'Project plan', link: '/developers/reference/plan/' },
            { label: '.imag file format', link: '/developers/reference/design/file-format/' },
            {
              label: 'Architecture decisions',
              collapsed: true,
              items: [{ autogenerate: { directory: 'developers/reference/adr' } }],
            },
          ],
        },
      ],
    }),
  ],
});
