import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MobSourceView } from '$lib/api';
import { createMobSourceModel, HOVER_CLOSE_MS } from './mobSourceModel.svelte';

const young = { name: 'Young Atrox', species: 'Atrox', maturity: 'Young' };
const atrox = { name: 'Old Atrox', species: 'Atrox', maturity: 'Old' };
const daikiba = { display: 'Daikiba Mature', species: 'Daikiba', maturity: 'Mature' };

function reading(overrides: Partial<MobSourceView> = {}): MobSourceView {
	return {
		readingScreen: true,
		pin: null,
		typed: young,
		reading: atrox,
		continuity: null,
		...overrides,
	};
}

function rig(initial: MobSourceView | null, steerable?: boolean) {
	let current = $state<MobSourceView | null>(initial);
	const deps = {
		view: () => current,
		...(steerable === undefined ? {} : { steerable: () => steerable }),
		pin: vi.fn(async () => ({})),
		unpin: vi.fn(async () => true),
		setTyped: vi.fn(async () => ({})),
		clearTyped: vi.fn(async () => ({})),
		refresh: vi.fn(async () => {}),
	};
	const model = createMobSourceModel(deps);
	return {
		model,
		deps,
		set(view: MobSourceView | null) {
			current = view;
		},
	};
}

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
});

describe('expanding the control', () => {
	it('opens and closes for good on its toggle, and on Escape', () => {
		const { model } = rig(reading());
		expect(model.expanded).toBe(false);
		model.toggle();
		expect(model.expanded).toBe(true);
		// Sticky: the pointer leaving does not close it.
		model.pointerLeave();
		vi.advanceTimersByTime(HOVER_CLOSE_MS * 4);
		expect(model.expanded).toBe(true);
		model.toggle();
		expect(model.expanded).toBe(false);
		model.toggle();
		model.close();
		expect(model.expanded).toBe(false);
	});

	it('opens on hover and closes a moment after the pointer leaves, unless it comes back', () => {
		const { model } = rig(reading());
		model.pointerEnter();
		expect(model.expanded).toBe(true);
		model.pointerLeave();
		vi.advanceTimersByTime(HOVER_CLOSE_MS - 1);
		model.pointerEnter();
		vi.advanceTimersByTime(HOVER_CLOSE_MS * 2);
		expect(model.expanded).toBe(true);
		model.pointerLeave();
		vi.advanceTimersByTime(HOVER_CLOSE_MS);
		expect(model.expanded).toBe(false);
	});

	it('keeps a hover-opened panel open when its toggle is clicked', () => {
		const { model } = rig(reading());
		model.pointerEnter();
		model.toggle();
		expect(model.expanded).toBe(true);
		model.pointerLeave();
		vi.advanceTimersByTime(HOVER_CLOSE_MS * 4);
		expect(model.expanded).toBe(true);
		model.toggle();
		expect(model.expanded).toBe(false);
	});

	it('stays open while the player works in it, whatever the pointer does', () => {
		const { model } = rig(reading());
		model.pointerEnter();
		model.hold();
		model.pointerLeave();
		vi.advanceTimersByTime(HOVER_CLOSE_MS * 4);
		expect(model.expanded).toBe(true);
	});

	it('never expands while the control is the typed mob alone', () => {
		const off = rig(reading({ readingScreen: false, reading: null }));
		expect(off.model.steered).toBe(false);
		off.model.toggle();
		off.model.pointerEnter();
		expect(off.model.expanded).toBe(false);
		// Nor when the control is told it cannot steer.
		const fixed = rig(reading(), false);
		expect(fixed.model.steered).toBe(false);
		fixed.model.toggle();
		expect(fixed.model.expanded).toBe(false);
	});

	it('steers a steered view unless told otherwise', () => {
		const open = rig(reading());
		expect(open.model.steered).toBe(true);
		open.model.toggle();
		expect(open.model.expanded).toBe(true);
	});

	it('shows the typed input in the panel while steered, in the strip while nothing is typed otherwise', () => {
		const steered = rig(reading());
		expect(steered.model.inputShown).toBe(false);
		steered.model.toggle();
		expect(steered.model.inputShown).toBe(true);
		const typed = rig(reading({ readingScreen: false, reading: null }));
		expect(typed.model.inputShown).toBe(false);
		typed.set(reading({ readingScreen: false, reading: null, typed: null }));
		expect(typed.model.inputShown).toBe(true);
	});
});

describe('steering the source', () => {
	it('pins what is shown, naming it so a moved-on reading is refused', async () => {
		const { model, deps } = rig(reading());
		expect(await model.pinShown()).toBe(true);
		expect(deps.pin).toHaveBeenCalledWith({ kind: 'shown', creature: atrox });
		expect(deps.refresh).toHaveBeenCalledTimes(1);
	});

	it('releases the pin', async () => {
		const { model, deps } = rig(
			reading({ readingScreen: false, reading: null, pin: { creature: atrox, by: 'read' } }),
		);
		expect(await model.release()).toBe(true);
		expect(deps.unpin).toHaveBeenCalledTimes(1);
	});

	it('holds a typed pick while steered until the player pins it or keeps it as the backstop', async () => {
		const { model, deps } = rig(reading());
		model.toggle();
		expect(await model.choose(daikiba)).toBe(true);
		expect(model.draft).toEqual(daikiba);
		expect(deps.setTyped).not.toHaveBeenCalled();
		expect(deps.pin).not.toHaveBeenCalled();

		expect(await model.pinDraft()).toBe(true);
		expect(deps.pin).toHaveBeenCalledWith({
			kind: 'typed',
			species: 'Daikiba',
			maturity: 'Mature',
		});
		expect(model.draft).toBeNull();

		await model.choose({ display: 'Atrox', species: 'Atrox', maturity: '' });
		expect(await model.backstopDraft()).toBe(true);
		expect(deps.setTyped).toHaveBeenCalledWith('Atrox', '');
		expect(model.draft).toBeNull();
	});

	it('takes a pick as the typed mob at once when not steered, as it always did', async () => {
		const { model, deps } = rig(reading({ readingScreen: false, reading: null, typed: null }));
		expect(await model.choose(daikiba)).toBe(true);
		expect(deps.setTyped).toHaveBeenCalledWith('Daikiba', 'Mature');
		expect(model.draft).toBeNull();
	});

	it('edits the backstop: clears it', async () => {
		const { model, deps } = rig(reading());
		expect(await model.clearTyped()).toBe(true);
		expect(deps.clearTyped).toHaveBeenCalledTimes(1);
	});

	it('refuses every other write while one is saving, and reports a failure', async () => {
		const { model, deps } = rig(reading());
		let finish: () => void = () => {};
		deps.pin.mockImplementationOnce(
			() =>
				new Promise((resolve) => {
					finish = () => resolve({});
				}),
		);
		const pinning = model.pinShown();
		expect(model.busy).toBe('pin');
		expect(await model.release()).toBe(false);
		expect(await model.clearTyped()).toBe(false);
		expect(deps.unpin).not.toHaveBeenCalled();
		expect(deps.clearTyped).not.toHaveBeenCalled();
		finish();
		expect(await pinning).toBe(true);
		expect(model.busy).toBeNull();

		deps.unpin.mockRejectedValueOnce(new Error('The mob being read has changed; pin it again'));
		expect(await model.release()).toBe(false);
		expect(model.error).toBe('The mob being read has changed; pin it again');
		model.dismissError();
		expect(model.error).toBeNull();
	});

	it('has nothing to pin when nothing is shown', async () => {
		const { model, deps } = rig(reading({ reading: null, typed: null }));
		expect(await model.pinShown()).toBe(false);
		expect(deps.pin).not.toHaveBeenCalled();
	});
});
