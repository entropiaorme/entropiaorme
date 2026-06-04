import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// Standalone Vitest config: deliberately does NOT load the SvelteKit Vite
// plugin (it does not run cleanly under Vitest), so the `$lib` alias that the
// plugin normally provides is declared explicitly here. Pure-TS logic modules
// run under the `node` environment; the few suites needing a DOM (window /
// localStorage, e.g. preferences) opt in per-file with a
// `// @vitest-environment happy-dom` docblock. TZ is pinned to UTC so the
// date-formatting suites are deterministic across machines and CI.
export default defineConfig({
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
		include: ['src/**/*.test.ts'],
		coverage: {
			provider: 'v8',
			reporter: ['text', 'html'],
			include: [
				'src/lib/utils/format.ts',
				'src/lib/statsRegistry.ts',
				'src/lib/statsCustomisation.ts',
				'src/lib/news.ts',
				'src/lib/activityArchive.ts',
				'src/lib/preferences.ts',
			],
		},
	},
});
