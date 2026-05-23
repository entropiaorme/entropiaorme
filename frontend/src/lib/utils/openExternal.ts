import { open as shellOpen } from '@tauri-apps/plugin-shell';
import type { Action } from 'svelte/action';

type TauriWindow = Window & { __TAURI_INTERNALS__?: unknown };

/**
 * Open an external URL in the OS browser / default handler.
 *
 * In the Tauri runtime a plain `target="_blank"` anchor does not reach the OS
 * browser, so external links must be handed to the shell `open` API (the same
 * path the interactive guide uses). Falls back to `window.open` when running
 * outside Tauri, e.g. a plain browser during development. No-ops on an empty
 * href so callers can pass optional values without guarding.
 */
export async function openExternalUrl(href: string | null | undefined): Promise<void> {
	if (!href) return;
	const tauriWindow = window as TauriWindow;
	if (tauriWindow.__TAURI_INTERNALS__) {
		try {
			await shellOpen(href);
			return;
		} catch (error) {
			console.warn('[external-link] shell open failed, falling back to window.open:', error);
		}
	}
	window.open(href, '_blank', 'noopener,noreferrer');
}

/**
 * True for hrefs that should be handed to the OS (web + mail), as opposed to
 * in-page anchors (`#...`) and internal app routes (`/...`) that navigate the
 * webview in place.
 */
export function isExternalHref(href: string): boolean {
	const trimmed = href.trim().toLowerCase();
	return (
		trimmed.startsWith('http:') ||
		trimmed.startsWith('https:') ||
		trimmed.startsWith('mailto:')
	);
}

/**
 * Svelte action for containers of rendered HTML (e.g. `{@html}` markdown).
 * Delegates click handling: external links inside the node are routed to the
 * OS browser; in-page anchors and internal routes navigate normally. Used as
 * an action rather than an element handler so the host element does not need
 * an interactive ARIA role (the anchors themselves remain keyboard-accessible:
 * Enter on a focused link fires a click that bubbles here).
 */
export const delegateExternalLinks: Action<HTMLElement> = (node) => {
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
