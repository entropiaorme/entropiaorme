import { describe, expect, it, vi } from 'vitest';
import type { MobSourceView } from '$lib/api';
import { createMobSourceModel } from './mobSourceModel.svelte';

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

describe('what the control is', () => {
	it('steers while the screen is read, unless told it cannot', () => {
		expect(rig(reading()).model.steered).toBe(true);
		expect(rig(reading({ readingScreen: false, reading: null })).model.steered).toBe(false);
		expect(rig(reading(), false).model.steered).toBe(false);
	});

	it('keeps its typed field in the readout only while it is the typed mob alone and none is typed', () => {
		// Steered, typing is in the control's sub-window.
		expect(rig(reading()).model.inputShown).toBe(false);
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

	it('pins a mob typed in its sub-window, or keeps it as the backstop', async () => {
		const { model, deps } = rig(reading());
		expect(await model.pinTyped('Daikiba', 'Mature')).toBe(true);
		expect(deps.pin).toHaveBeenCalledWith({
			kind: 'typed',
			species: 'Daikiba',
			maturity: 'Mature',
		});
		// A mob with no maturity is pinned with none.
		await model.pinTyped('Atrox', '');
		expect(deps.pin).toHaveBeenLastCalledWith({ kind: 'typed', species: 'Atrox', maturity: null });
		expect(deps.setTyped).not.toHaveBeenCalled();

		expect(await model.backstop('Atrox', '')).toBe(true);
		expect(deps.setTyped).toHaveBeenCalledWith('Atrox', '');
		expect(deps.refresh).toHaveBeenCalledTimes(3);
	});

	it("takes a pick from the readout's own field as the typed mob at once", async () => {
		const { model, deps } = rig(reading({ readingScreen: false, reading: null, typed: null }));
		expect(await model.choose(daikiba)).toBe(true);
		expect(deps.setTyped).toHaveBeenCalledWith('Daikiba', 'Mature');
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
