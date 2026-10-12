// SPDX-License-Identifier: AGPL-3.0-or-later
// Where the site is published. This is the ONLY place that knows the base
// path. Content links are root-relative ("/developers/…"), and
// plugins/remark-base-links.mjs prefixes BASE at build time. To move to a
// custom domain, set SITE to it and BASE to '/'.
export const SITE = 'https://giosuel.github.io';
export const BASE = '/imaginarium';
