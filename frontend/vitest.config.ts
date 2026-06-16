import { fileURLToPath } from 'node:url';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vitest/config';

// Standalone Vitest config: deliberately does NOT load the SvelteKit Vite
// plugin (it does not run cleanly under Vitest), so the `$lib` alias that the
// plugin normally provides is declared explicitly here. The plain Svelte
// plugin compiles `.svelte` components and `.svelte.ts` runes modules for the
// component suites (it reads svelte.config.js, so runes stay enabled);
// svelteTesting() adds Testing Library's auto-cleanup and browser-condition
// resolution. Pure-TS logic modules run under the `node` environment; suites
// needing a DOM (component tests, window / localStorage seams) opt in
// per-file with a `// @vitest-environment happy-dom` docblock. TZ is pinned
// to UTC so the date-formatting suites are deterministic across machines and
// CI.
export default defineConfig({
	plugins: [svelte(), svelteTesting()],
	// vite.config.ts inlines the backend port the same way for the real build;
	// the suites pin URLs against this fixed value rather than a machine's env.
	define: {
		'import.meta.env.ENTROPIAORME_BACKEND_PORT': JSON.stringify('8421'),
	},
	resolve: {
		alias: {
			$lib: fileURLToPath(new URL('./src/lib', import.meta.url)),
		},
	},
	test: {
		environment: 'node',
		env: {
			TZ: 'UTC',
		},
		// `src/**` covers the app suites; the `src-tauri` entry pulls in the
		// dev-tooling build-script tests (e.g. build-dev-config), which live
		// beside the script they exercise rather than under `src/`.
		include: ['src/**/*.test.ts', 'src-tauri/entropia-orme/*.test.ts'],
		coverage: {
			provider: 'v8',
			reporter: ['text', 'html'],
			include: [
				'src/lib/motion/testMotion.ts',
				'src/lib/utils/format.ts',
				'src/lib/statsRegistry.ts',
				'src/lib/statsCustomisation.ts',
				'src/lib/news.ts',
				'src/lib/activityArchive.ts',
				'src/lib/preferences.ts',
				'src/lib/api/client.ts',
				'src/lib/api/index.ts',
				'src/lib/realtime/useVisiblePoll.ts',
				'src/lib/realtime/eventRelay.ts',
				'src/lib/stores/trackingStore.ts',
				'src/lib/stores/scanStore.ts',
			],
		},
	},
});
