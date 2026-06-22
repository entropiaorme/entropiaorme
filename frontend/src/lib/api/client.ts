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

import { invoke } from '@tauri-apps/api/core';
import createClient, { type Middleware } from 'openapi-fetch';
import type { paths } from './schema';

/** The backend's nominal loopback base. The IPC facade (`tauriFetch`) parses
 * these URLs for their path and query only; the origin is never dialled (the
 * in-process command carries the request), so the port here is nominal. The
 * generated paths carry the `/api` prefix themselves, so the base is the bare
 * origin. */
const API_ORIGIN = `http://127.0.0.1:${import.meta.env.ENTROPIAORME_BACKEND_PORT}`;

const API_BASE = `${API_ORIGIN}/api`;

export class ApiError extends Error {
	constructor(
		public status: number,
		message: string,
	) {
		super(message);
		this.name = 'ApiError';
	}
}

/** The manual-scan capture preview PNG for a page, as a base64 `data:` URL for
 * an `<img>` `src`. The route returns raw image bytes (deliberately excluded
 * from the OpenAPI schema), so it rides its own `capture_png` command rather
 * than the JSON `api_request` envelope, dispatched through the same in-process
 * router (no socket). */
export async function manualSkillScanCapturePng(page: number): Promise<string> {
	const encoded = await invoke<string>('capture_png', { page });
	return `data:image/png;base64,${encoded}`;
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
type ApiResponseWire = {
	status: number;
	statusText: string;
	headers: [string, string][];
	body: string;
};

/* The IPC transport that replaces the loopback HTTP hop. Every backend call
 * the openapi-fetch client (and the hand-rolled `request` below) makes is
 * routed through the `api_request` Tauri command, which dispatches the
 * in-process axum router with no socket. It is a drop-in for the global
 * `fetch` the client used: same Request in, same Response out, so the call
 * sites, the generated `paths` types, the error middleware, and `unwrap` are
 * all unchanged. No frontend traffic remains on the loopback origin: the
 * `/api/events` stream now rides native Tauri events, and the capture-PNG
 * `<img>` src its own `capture_png` command. The loopback listener is removed
 * once the socket has no remaining web tenant. */
export async function tauriFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
	const req = input instanceof Request ? input : new Request(input, init);
	const url = new URL(req.url);
	const method = req.method.toUpperCase();
	const headers: [string, string][] = [];
	req.headers.forEach((value, key) => {
		headers.push([key, value]);
	});
	const body = method === 'GET' || method === 'HEAD' ? undefined : await req.text();

	const res = await invoke<ApiResponseWire>('api_request', {
		request: { method, path: url.pathname + url.search, headers, body },
	});

	// A 204/304 must be constructed with a null body or the Response constructor
	// throws; the ETag-bearing hydration GETs depend on the 304 path.
	const nullBody = res.status === 204 || res.status === 304;
	return new Response(nullBody ? null : res.body, {
		status: res.status,
		statusText: res.statusText,
		headers: new Headers(res.headers),
	});
}

export const client = createClient<paths>({
	baseUrl: API_ORIGIN,
	headers: { 'Content-Type': 'application/json' },
	fetch: tauriFetch,
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
	const resp = await tauriFetch(url, {
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
