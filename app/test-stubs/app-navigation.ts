// Vitest stand-in for SvelteKit's `$app/navigation`, which exists only under
// the SvelteKit Vite plugin the unit suites do not load (see vitest.config.ts).
// Navigation is inert here; a suite asserting on it mocks this module.
export async function goto(_url: string | URL, _opts?: unknown): Promise<void> {}
