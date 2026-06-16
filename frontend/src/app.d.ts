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
		// Build-time flag ('1' only in the e2e's own Vite build) that forces
		// JS-driven chart tweens to settle instantly so visual-regression
		// baselines capture the settled end-state. Unset (and so '') in every
		// shipped build. See frontend/src/lib/motion/testMotion.ts.
		readonly E2E_FREEZE_TWEENS: string;
	}

	interface ImportMeta {
		readonly env: ImportMetaEnv;
	}
}

export {};
