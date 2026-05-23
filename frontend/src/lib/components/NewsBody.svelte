<script lang="ts">
	import { Marked } from 'marked';
	import { isExternalHref, externalLinks } from '$lib/utils/openExternal';

	let { markdown }: { markdown: string } = $props();

	const md = new Marked({ gfm: true, breaks: false });

	const ALLOWED_LINK_SCHEMES = ['http:', 'https:', 'mailto:'];

	function isSafeLinkHref(href: string): boolean {
		const trimmed = href.trim().toLowerCase();
		if (trimmed.startsWith('#') || trimmed.startsWith('/')) return true;
		return ALLOWED_LINK_SCHEMES.some((s) => trimmed.startsWith(s));
	}

	function escapeHtml(s: string): string {
		return s
			.replace(/&/g, '&amp;')
			.replace(/</g, '&lt;')
			.replace(/>/g, '&gt;')
			.replace(/"/g, '&quot;')
			.replace(/'/g, '&#39;');
	}

	md.use({
		renderer: {
			// Drop all raw HTML in source content. Defence in depth on top of CSP:
			// inline scripts are blocked by `script-src 'self'`, external images by
			// `img-src 'self' data:`, frames by `default-src`. This removes the
			// raw-HTML escape route entirely.
			html() {
				return '';
			},
			// Allowlist link schemes. CSP already blocks inline script execution,
			// but `javascript:` href navigation in webviews is historically uneven;
			// explicit scheme allowlisting closes that gap and mirrors the
			// raw-HTML strip above.
			link({ href, title, tokens }) {
				const text = this.parser.parseInline(tokens);
				if (!isSafeLinkHref(href)) {
					return text;
				}
				const titleAttr = title ? ` title="${escapeHtml(title)}"` : '';
				// External links (web + mail) carry target/rel for browser-context
				// correctness; in the Tauri runtime the delegated click handler
				// below routes them to the OS browser via the shell open API.
				// In-page anchors (#...) and internal routes (/...) stay in-app.
				const targetAttr = isExternalHref(href)
					? ' target="_blank" rel="noopener noreferrer"'
					: '';
				return `<a href="${escapeHtml(href)}"${titleAttr}${targetAttr}>${text}</a>`;
			},
			// Drop images entirely. CSP `img-src 'self' data:` would already block
			// remote loads, but rendering nothing is unambiguous and avoids any
			// surprise from data-URL payloads. News content is text-first.
			image() {
				return '';
			},
		},
	});

	let html = $derived(md.parse(markdown) as string);
</script>

<!-- Rendered markdown is injected via {@html}, so its anchors cannot carry
     Svelte handlers directly. The action delegates clicks on external links to
     the OS browser (mirroring the interactive guide); in-page anchors and
     internal routes navigate the webview in place. -->
<div class="prose" use:externalLinks>
	{@html html}
</div>

<style>
	.prose {
		font-size: 0.9rem;
		line-height: 1.65;
		color: var(--color-text-secondary);
	}
	.prose :global(h1),
	.prose :global(h2),
	.prose :global(h3),
	.prose :global(h4) {
		color: var(--color-text);
		font-weight: 500;
		letter-spacing: -0.012em;
		line-height: 1.25;
		margin: 1.25rem 0 0.5rem;
	}
	.prose :global(h1) {
		font-size: 1.25rem;
	}
	.prose :global(h2) {
		font-size: 1.1rem;
	}
	.prose :global(h3),
	.prose :global(h4) {
		font-size: 1rem;
	}
	.prose :global(p) {
		margin: 0.75rem 0;
	}
	.prose :global(ul),
	.prose :global(ol) {
		margin: 0.75rem 0;
		padding-left: 1.4rem;
	}
	.prose :global(li) {
		margin: 0.25rem 0;
	}
	.prose :global(a) {
		color: var(--color-accent);
		text-decoration: none;
		border-bottom: 1px solid color-mix(in oklab, var(--color-accent) 35%, transparent);
		transition:
			color var(--duration-base) var(--ease-out),
			border-color var(--duration-base) var(--ease-out);
	}
	.prose :global(a:hover) {
		color: var(--color-accent-hover);
		border-bottom-color: var(--color-accent);
	}
	.prose :global(code) {
		font-family: var(--font-mono, ui-monospace, 'Cascadia Code', monospace);
		font-size: 0.85em;
		padding: 0.1rem 0.35rem;
		border-radius: var(--radius-sm, 4px);
		background: color-mix(in oklab, var(--color-surface) 60%, transparent);
		border: 1px solid color-mix(in oklab, var(--color-border) 60%, transparent);
	}
	.prose :global(pre) {
		margin: 0.875rem 0;
		padding: 0.875rem 1rem;
		border-radius: var(--radius-md);
		background: color-mix(in oklab, var(--color-surface) 70%, transparent);
		border: 1px solid color-mix(in oklab, var(--color-border) 70%, transparent);
		overflow-x: auto;
	}
	.prose :global(pre code) {
		background: transparent;
		border: none;
		padding: 0;
		font-size: 0.85rem;
		line-height: 1.55;
	}
	.prose :global(blockquote) {
		margin: 0.875rem 0;
		padding: 0.5rem 0.875rem;
		border-left: 2px solid color-mix(in oklab, var(--color-accent) 55%, transparent);
		background: color-mix(in oklab, var(--color-accent) 6%, transparent);
		color: var(--color-text);
	}
	.prose :global(hr) {
		margin: 1.25rem 0;
		border: 0;
		border-top: 1px solid color-mix(in oklab, var(--color-border) 70%, transparent);
	}
	.prose :global(strong) {
		color: var(--color-text);
		font-weight: 600;
	}
</style>
