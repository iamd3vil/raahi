import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

// Needed by svelte-check / editor tooling; vite picks up the plugin directly.
export default {
  preprocess: vitePreprocess(),
};
