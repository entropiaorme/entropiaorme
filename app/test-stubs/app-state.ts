// Vitest stand-in for SvelteKit's `$app/state` (see app-navigation.ts): the
// page state at the app root. A suite needing another route mocks this module.
export const page = { url: new URL('http://localhost/') };
