import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Frontend port: defaults to 5173 when ENTROPIAORME_FRONTEND_PORT is unset.
// Setting the env var at process start lets multiple instances of the app
// run concurrently on the same machine without port collisions.
const port = parseInt(process.env.ENTROPIAORME_FRONTEND_PORT ?? '5173', 10);

// Backend port: injected into the client bundle so api.ts can address the
// backend on its env-driven port without a hardcoded fallback. Process env
// is available here because just sources .env.local before invoking vite;
// `define` substitutes the value as a string literal at build time, which
// Svelte client code reads via import.meta.env.ENTROPIAORME_BACKEND_PORT.
const backendPort = process.env.ENTROPIAORME_BACKEND_PORT ?? '8421';

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
	server: {
		port,
		strictPort: true
	},
	define: {
		'import.meta.env.ENTROPIAORME_BACKEND_PORT': JSON.stringify(backendPort)
	}
});
