import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	plugins: [
		svelte({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			}
		})
	],
	resolve: {
		// Resolve `svelte` to its client build so `mount`/lifecycles work in jsdom.
		conditions: ['browser'],
		alias: [
			// Route components use SvelteKit's `$app` modules; point them at test doubles.
			{ find: '$app', replacement: fileURLToPath(new URL('./src/test/mocks/app', import.meta.url)) },
			{ find: '$lib', replacement: fileURLToPath(new URL('./src/lib', import.meta.url)) }
		]
	},
	test: {
		environment: 'jsdom',
		globals: true,
		setupFiles: ['./src/test/setup.ts'],
		include: ['src/**/*.test.ts'],
		exclude: ['node_modules/**', 'e2e/**']
	}
});
