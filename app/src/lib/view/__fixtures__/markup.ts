/**
 * Rendered markup as a page shows it, for comparing a surface across
 * versions of its component: the framework's empty block anchors (which
 * move whenever a block is added around unchanged content) and the
 * whitespace between tags are dropped, and runs of whitespace collapse.
 * A dash is written as its entity, so a stored copy of the markup carries
 * none of the typography the authoring rules keep out of new text.
 */
export function shownMarkup(html: string): string {
	return html
		.replace(/<!--[^>]*-->/g, '')
		.replace(/>\s+</g, '><')
		.replace(/\s+/g, ' ')
		.replace(/\u2014/g, '&mdash;')
		.trim();
}
