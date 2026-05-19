import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Frontend port: defaults to 5173 when ENTROPIAORME_FRONTEND_PORT is unset.
// Setting the env var at process start lets multiple instances of the app
// run concurrently on the same machine without port collisions.
const port = parseInt(process.env.ENTROPIAORME_FRONTEND_PORT ?? '5173', 10);

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
	server: {
		port,
		strictPort: true
	}
});
