import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

// Read a port from env, validate range, fall back to a default when unset.
// Fails fast at config time with a descriptive error so an invalid value
// surfaces during `vite` startup rather than producing NaN binds or
// malformed URLs in the resulting bundle. Mirrors the original Python
// implementation's _read_port shape so both halves of the chain enforce the same contract.
function readPort(name: string, defaultValue: number): number {
	const raw = (process.env[name] ?? String(defaultValue)).trim();
	const port = Number(raw);
	if (!Number.isInteger(port) || port < 1 || port > 65535) {
		throw new Error(`${name} must be an integer between 1 and 65535`);
	}
	return port;
}

// Frontend port: bound by Vite's dev server. Backend port: injected into
// the client bundle as import.meta.env.ENTROPIAORME_BACKEND_PORT so api.ts
// addresses the backend on its env-driven port without a hardcoded fallback.
// Process env is available here because just sources .env.local before
// invoking vite; `define` substitutes the value as a string literal at
// build time.
const port = readPort('ENTROPIAORME_FRONTEND_PORT', 5173);
const backendPort = readPort('ENTROPIAORME_BACKEND_PORT', 8421);

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
	server: {
		port,
		strictPort: true,
	},
	define: {
		'import.meta.env.ENTROPIAORME_BACKEND_PORT': JSON.stringify(String(backendPort)),
		// Forces the JS-driven chart tweens to settle instantly (visual-regression
		// determinism). Set to '1' only by the e2e's own Vite build; unset (so '')
		// in every shipped build, where the freeze branch then folds to a static
		// false and tree-shakes out. See frontend/src/lib/motion/testMotion.ts.
		'import.meta.env.E2E_FREEZE_TWEENS': JSON.stringify(process.env.E2E_FREEZE_TWEENS ?? ''),
	},
});
