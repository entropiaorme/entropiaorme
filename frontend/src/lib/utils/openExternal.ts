import { open as shellOpen } from '@tauri-apps/plugin-shell';
import type { Action } from 'svelte/action';

type TauriWindow = Window & { __TAURI_INTERNALS__?: unknown };

/**
 * Open an external URL in the OS browser / default handler.
 *
 * In the Tauri runtime a plain `target="_blank"` anchor does not reach the OS
 * browser, so external links must be handed to the shell `open` API (the same
 * path the interactive guide uses). Outside Tauri (e.g. a plain browser during
 * development) it uses `window.open` instead. No-ops on an empty or
 * non-allowlisted href, so callers can pass optional / untrusted values without
 * guarding.
 */
export async function openExternalUrl(href: string | null | undefined): Promise<void> {
	if (!href) return;
	const trimmed = href.trim();
	// Self-enforce the scheme allowlist so a non-allowlisted href can never
	// reach shellOpen or window.open, whatever the caller passed.
	if (!isExternalHref(trimmed)) return;
	const tauriWindow = window as TauriWindow;
	if (tauriWindow.__TAURI_INTERNALS__) {
		try {
			await shellOpen(trimmed);
		} catch (error) {
			// Do not downgrade to window.open on failure: a scope rejection
			// must not be bypassed, and window.open does not reach the OS
			// browser from the Tauri webview anyway.
			console.warn('[external-link] shell open failed:', error);
		}
		return;
	}
	window.open(trimmed, '_blank', 'noopener,noreferrer');
}

/**
 * True for hrefs that should be handed to the OS (web + mail), as opposed to
 * in-page anchors (`#...`) and internal app routes (`/...`) that navigate the
 * webview in place.
 */
export function isExternalHref(href: string): boolean {
	const trimmed = href.trim().toLowerCase();
	return (
		trimmed.startsWith('http:') || trimmed.startsWith('https:') || trimmed.startsWith('mailto:')
	);
}

/**
 * Svelte action: the single app-wide pattern for opening external links. Apply
 * it to an external-link anchor, or to a container of rendered HTML (e.g.
 * `{@html}` markdown). Click handling is delegated from the node: external
 * links route to the OS browser via {@link openExternalUrl}; in-page anchors
 * (`#...`) and internal routes (`/...`) navigate normally. As an action rather
 * than an element click handler the host needs no interactive ARIA role, and
 * anchors stay keyboard-accessible (Enter on a focused link fires a click that
 * reaches this listener).
 */
export const externalLinks: Action<HTMLElement> = (node) => {
	function handleClick(event: MouseEvent) {
		const anchor = (event.target as HTMLElement | null)?.closest('a');
		const href = anchor?.getAttribute('href');
		if (!href || !isExternalHref(href)) return;
		event.preventDefault();
		event.stopPropagation();
		void openExternalUrl(href);
	}
	node.addEventListener('click', handleClick);
	return {
		destroy() {
			node.removeEventListener('click', handleClick);
		},
	};
};
