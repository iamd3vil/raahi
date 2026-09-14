import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://raahi.sarat.dev',
  integrations: [
    starlight({
      title: 'Raahi',
      description: 'Documentation for Raahi, a fast, self-hosted reverse proxy and API gateway.',
      logo: {
        dark: './src/assets/logo-dark.svg',
        light: './src/assets/logo-light.svg',
        alt: 'Raahi',
        replacesTitle: true,
      },
      favicon: '/favicon.svg',
      customCss: ['./src/styles/custom.css'],
      lastUpdated: true,
      social: [
        {
          icon: 'github',
          label: 'Source code',
          href: 'https://github.com/iamd3vil/raahi',
        },
      ],
      editLink: {
        baseUrl: 'https://github.com/iamd3vil/raahi/edit/main/site/',
      },
      head: [
        { tag: 'meta', attrs: { name: 'theme-color', content: '#0b0d0c' } },
        { tag: 'meta', attrs: { property: 'og:site_name', content: 'Raahi documentation' } },
      ],
      sidebar: [
        { label: 'Start here', items: [
          { label: 'Introduction', slug: 'index' },
          { label: 'Installation', slug: 'start-here/installation' },
          { label: 'Quickstart', slug: 'start-here/quickstart' },
        ]},
        { label: 'Concepts', items: [{ autogenerate: { directory: 'concepts' } }] },
        { label: 'Guides', items: [{ autogenerate: { directory: 'guides' } }] },
        { label: 'Operations', items: [{ autogenerate: { directory: 'operations' } }] },
        { label: 'API', items: [{ autogenerate: { directory: 'api' } }] },
        { label: 'Contributing', items: [{ autogenerate: { directory: 'contributing' } }] },
      ],
    }),
  ],
});
