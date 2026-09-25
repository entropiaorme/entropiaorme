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
	resolve: {
		alias: {
			$lib: fileURLToPath(new URL('./src/lib', import.meta.url)),
			// SvelteKit's runtime modules, stubbed so a suite can mount a route
			// (or a component that navigates) without the plugin.
			'$app/navigation': fileURLToPath(new URL('./test-stubs/app-navigation.ts', import.meta.url)),
			'$app/state': fileURLToPath(new URL('./test-stubs/app-state.ts', import.meta.url)),
		},
	},
	test: {
		environment: 'node',
		env: {
			TZ: 'UTC',
		},
		// `src/**` covers the app suites; the `src-tauri` entry pulls in the
		// dev-tooling build-script tests (e.g. build-dev-config), which live
		// beside the script they exercise rather than under `src/`; the `e2e`
		// entry covers the native-shell harness helpers' own unit tests (e.g.
		// ensureViewport's inner-viewport recovery logic), which live beside the
		// helper they exercise.
		include: ['src/**/*.test.ts', 'src-tauri/entropia-orme/*.test.ts', 'e2e/helpers/*.test.mjs'],
		coverage: {
			provider: 'v8',
			reporter: ['text', 'html'],
			// Directory-based instrumentation over the tested layers (feature
			// modules, view models, window helpers, realtime plumbing, the API
			// seam, the runes state modules) plus the standalone pure-logic
			// modules, so a new module in a tested layer is instrumented from the
			// moment it lands rather than only once someone remembers to list it.
			// `.svelte` components stay excluded (they are exercised through the
			// module suites and the native-shell e2e, not unit-instrumented), as
			// do generated files and test files.
			include: [
				'src/lib/features/**',
				'src/lib/view/**',
				'src/lib/windows/**',
				'src/lib/realtime/**',
				'src/lib/api/**',
				'src/lib/stores/**',
				'src/lib/*.svelte.ts',
				'src/lib/motion/testMotion.ts',
				'src/lib/utils/format.ts',
				'src/lib/statsRegistry.ts',
				'src/lib/preferences.ts',
			],
			exclude: [
				'**/*.svelte',
				'**/*.test.ts',
				'**/__fixtures__/**',
				'**/*.d.ts',
				'src/lib/api/commands.gen.ts',
			],
		},
	},
});
