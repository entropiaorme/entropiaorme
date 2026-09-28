import type { Action } from 'svelte/action';

/**
 * Move a node to the document body for its lifetime.
 *
 * Floating layers (menus, popovers, dialogs) belong to the document layer,
 * not their owner's stacking context. A `position: fixed` element is only
 * viewport-relative while no ancestor establishes a containing block for it,
 * and a transform, a filter, or a `backdrop-filter` (the dashboard `.panel`
 * carries one) all do; an `overflow` ancestor would clip it besides. Moving
 * the node to body escapes all of these. Svelte's delegated event handlers
 * are attached at the document too, so handlers inside the node keep working.
 *
 * `enabled` is read once at mount: a node that should portal conditionally
 * decides before it is created.
 */
export const portal: Action<HTMLElement, boolean | undefined> = (node, enabled = true) => {
	if (enabled) document.body.appendChild(node);
	return {
		destroy() {
			if (enabled) node.remove();
		},
	};
};
