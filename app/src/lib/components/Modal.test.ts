// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { beforeAll, describe, expect, it } from 'vitest';
import Modal from './Modal.svelte';

// happy-dom has no Web Animations API, and Svelte drives transition
// completion (and outro element removal) through `animation.onfinish`; the
// stub finishes instantly on a microtask so transitioned branches settle.
beforeAll(() => {
	Element.prototype.animate = function animate() {
		const animation = {
			cancel() {},
			finish() {},
			effect: null,
			currentTime: 0,
			playState: 'finished',
			onfinish: null as (() => void) | null,
			oncancel: null as (() => void) | null,
		};
		queueMicrotask(() => animation.onfinish?.());
		return animation as unknown as Animation;
	};
});

const twoButtons = createRawSnippet(() => ({
	render: () => '<div><button>First action</button><button>Second action</button></div>',
}));

describe('dialog semantics', () => {
	it('renders an aria-modal dialog and moves focus onto the panel', () => {
		render(Modal, { props: { open: true, title: 'Confirm', children: twoButtons } });

		const dialog = screen.getByRole('dialog');
		expect(dialog.getAttribute('aria-modal')).toBe('true');
		expect(document.activeElement).toBe(dialog);
	});

	it('still closes on Escape', async () => {
		render(Modal, { props: { open: true, children: twoButtons } });

		await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
		await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
	});
});

describe('focus trap', () => {
	it('wraps Tab from the last focusable back to the first', async () => {
		render(Modal, { props: { open: true, title: 'Confirm', children: twoButtons } });

		// With a title, the header close button is the first focusable.
		const first = screen.getByLabelText('Close');
		const last = screen.getByText('Second action');
		last.focus();

		await fireEvent.keyDown(last, { key: 'Tab' });
		expect(document.activeElement).toBe(first);
	});

	it('wraps Shift+Tab from the first focusable to the last', async () => {
		render(Modal, { props: { open: true, title: 'Confirm', children: twoButtons } });

		const first = screen.getByLabelText('Close');
		const last = screen.getByText('Second action');
		first.focus();

		await fireEvent.keyDown(first, { key: 'Tab', shiftKey: true });
		expect(document.activeElement).toBe(last);
	});

	it('sends Tab into the panel when focus starts on the panel itself', async () => {
		render(Modal, { props: { open: true, title: 'Confirm', children: twoButtons } });

		const dialog = screen.getByRole('dialog');
		expect(document.activeElement).toBe(dialog);

		await fireEvent.keyDown(dialog, { key: 'Tab', shiftKey: true });
		expect(document.activeElement).toBe(screen.getByText('Second action'));
	});
});

describe('focus restore', () => {
	it('returns focus to the previously focused element on close', async () => {
		const outside = document.createElement('button');
		outside.textContent = 'Opener';
		document.body.appendChild(outside);
		try {
			const { rerender } = render(Modal, { props: { open: false, children: twoButtons } });

			outside.focus();
			await rerender({ open: true });
			expect(document.activeElement).toBe(screen.getByRole('dialog'));

			await rerender({ open: false });
			await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
			expect(document.activeElement).toBe(outside);
		} finally {
			outside.remove();
		}
	});
});

describe('document layer', () => {
	it('lives on the document body, outside an owner that would re-anchor fixed positioning', async () => {
		const { container, unmount } = render(Modal, {
			props: { open: true, title: 'Confirm', children: twoButtons },
		});

		const dialog = screen.getByRole('dialog');
		expect(container.contains(dialog)).toBe(false);
		expect(document.body.contains(dialog)).toBe(true);

		// The backdrop still dismisses: handlers survive the move to body.
		const backdrop = dialog.parentElement as HTMLElement;
		await fireEvent.click(backdrop);
		await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

		unmount();
		expect(document.body.querySelector('[role="dialog"]')).toBeNull();
	});
});
