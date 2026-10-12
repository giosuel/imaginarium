// SPDX-License-Identifier: AGPL-3.0-or-later
// Remark plugin: prefixes the site base path to root-relative Markdown links
// ("/developers/architecture/" → "/imaginarium/developers/architecture/"), so
// content never hardcodes the base. External, protocol-relative ("//…") and
// already-prefixed links are left alone.

/** @param {string} base e.g. "/imaginarium" or "/" */
export function remarkBaseLinks(base) {
  const prefix = base.replace(/\/+$/, '');
  const rewrite = (url) => {
    if (!prefix || !url.startsWith('/') || url.startsWith('//')) return url;
    if (url === prefix || url.startsWith(`${prefix}/`)) return url;
    return `${prefix}${url}`;
  };
  const walk = (node) => {
    if ((node.type === 'link' || node.type === 'definition') && typeof node.url === 'string') {
      node.url = rewrite(node.url);
    }
    for (const child of node.children ?? []) walk(child);
  };
  return () => (tree) => walk(tree);
}
