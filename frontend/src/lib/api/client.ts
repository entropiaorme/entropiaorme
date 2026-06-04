/**
 * Generated-client plumbing for the backend API.
 *
 * `client` is an openapi-fetch instance typed by the generated `schema.d.ts`
 * (regenerated from the committed OpenAPI snapshot via `npm run gen:api`; the
 * CI freshness step keeps the two in lockstep). Every call site gets its path,
 * method, parameters, and request body verified against the backend's OpenAPI
 * contract at compile time.
 *
 * The error middleware reproduces the long-standing `request()` error
 * contract: any non-2xx response throws `ApiError(status, message)`, where
 * `message` prefers the FastAPI `detail` string when the body carries one, so
 * callers never see openapi-fetch's `{ data, error }` split.
 */

import createClient, { type Middleware } from 'openapi-fetch';
import type { paths } from './schema';

/** Loopback origin the Python backend listens on. The generated paths carry
 * the `/api` prefix themselves, so the client's base URL is the bare origin. */
const API_ORIGIN = `http://127.0.0.1:${import.meta.env.ENTROPIAORME_BACKEND_PORT}`;

const API_BASE = `${API_ORIGIN}/api`;

/** Server-sent-events stream the main-window relay subscribes to (see
 * `$lib/realtime/eventRelay`). Lives on the same loopback origin as every other
 * `/api/*` call, so it needs no separate CSP `connect-src` entry. */
export const EVENTS_STREAM_URL = `${API_BASE}/events`;

export class ApiError extends Error {
	constructor(
		public status: number,
		message: string,
	) {
		super(message);
		this.name = 'ApiError';
	}
}

/** Direct URL for the manual-scan capture preview PNG. Consumed as an `<img>`
 * `src`, not fetched as JSON, so it stays a hand-built URL outside the
 * generated client (the route is deliberately excluded from the OpenAPI
 * schema: it returns raw image bytes). */
export function manualSkillScanCapturePngUrl(page: number): string {
	return `${API_BASE}/scan/skills/capture/${page}`;
}

/* Turns every non-2xx response into a thrown ApiError before openapi-fetch
 * builds its `{ data, error }` result, preserving the legacy throwing
 * contract across the whole facade. */
const throwApiError: Middleware = {
	async onResponse({ response }) {
		if (!response.ok) {
			const text = await response.text().catch(() => response.statusText);
			let message = text || response.statusText;
			try {
				const parsed = JSON.parse(text);
				if (typeof parsed?.detail === 'string' && parsed.detail.trim()) {
					message = parsed.detail;
				}
			} catch {
				// Plain-text or non-JSON error body
			}
			throw new ApiError(response.status, message);
		}
		return undefined;
	},
};

/* The Content-Type seed reproduces the legacy request() header behaviour
 * byte-for-byte: it sent `Content-Type: application/json` on every call,
 * including bodyless GETs and POSTs, where openapi-fetch would otherwise
 * omit the header. No backend route reads it on a bodyless request, but
 * keeping the wire bytes identical costs one line. */
export const client = createClient<paths>({
	baseUrl: API_ORIGIN,
	headers: { 'Content-Type': 'application/json' },
});
client.use(throwApiError);

/**
 * Await an openapi-fetch call and return its payload as the facade's declared
 * type. The error middleware throws on every non-2xx, and every endpoint this
 * facade unwraps returns a non-empty 2xx JSON body (the void-returning
 * wrappers bypass unwrap), so `data` is present on the non-throwing path.
 * openapi-fetch would yield undefined for a 204 or empty body; no unwrapped
 * route emits one.
 *
 * The declared type may deliberately narrow the generated schema type: the
 * hand-written interfaces in `$lib/types/*` and the facade carry literal
 * unions (e.g. `'expense' | 'markup'`) where the spec types plain strings,
 * and they remain the authoritative frontend contract. This mirrors the
 * legacy `request<T>()`, which likewise asserted its return type over an
 * untyped `resp.json()`; the backend test suite pins the wire shapes the
 * assertion rests on.
 */
export async function unwrap<T>(call: Promise<{ data?: unknown }>): Promise<T> {
	const { data } = await call;
	return data as T;
}

/** Hand-rolled fetch against a backend route, kept for calls that cannot go
 * through the generated client. Throws the same `ApiError` on non-2xx. */
export async function request<T>(path: string, options?: RequestInit): Promise<T> {
	const url = `${API_BASE}${path}`;
	const resp = await fetch(url, {
		headers: { 'Content-Type': 'application/json' },
		...options,
	});

	if (!resp.ok) {
		const text = await resp.text().catch(() => resp.statusText);
		let message = text || resp.statusText;
		try {
			const parsed = JSON.parse(text);
			if (typeof parsed?.detail === 'string' && parsed.detail.trim()) {
				message = parsed.detail;
			}
		} catch {
			// Plain-text or non-JSON error body
		}
		throw new ApiError(resp.status, message);
	}

	return resp.json();
}
