# Raahi documentation site

The public documentation uses Astro and Starlight. Raahi itself is licensed under GPL-3.0-only.

```bash
cd site
npm install
npm run dev
npm run build
```

Astro writes the static site to `site/dist/`. `site/public/openapi.yaml` points to the API contract in `docs/openapi.yaml`.

Deploy the built site to Cloudflare Pages:

```bash
npx wrangler pages deploy dist --project-name raahi-docs
```

The production URL is `https://raahi.sarat.dev`.
