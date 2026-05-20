// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}

	// Augment Vite's ImportMetaEnv with project-specific build-time injections
	// (see frontend/vite.config.ts `define`). Keeps svelte-check honest about
	// the typed surface of import.meta.env reads.
	interface ImportMetaEnv {
		readonly ENTROPIAORME_BACKEND_PORT: string;
	}

	interface ImportMeta {
		readonly env: ImportMetaEnv;
	}
}

export {};
