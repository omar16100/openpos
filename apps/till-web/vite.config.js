import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  server: {
    // The worker and the bridge live in apps/shared, above this app's root.
    fs: { allow: ['..', '../shared'] },
    // OPFS sync access handles need a secure context, which localhost is.
    headers: {
      // Not required today and cheap to set: if a shared array buffer is ever
      // needed for the worker, its absence is discovered at the worst moment.
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'require-corp',
    },
  },
  // The wasm module is fetched at runtime, not bundled: it is the one artifact
  // that must be byte-identical to what was tested.
  build: { target: 'es2022' },
});
