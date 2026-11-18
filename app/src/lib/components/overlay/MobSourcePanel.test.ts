// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { OverlayMobSourceState } from '$lib/windows/overlayMenu';
import MobSourcePanel from './MobSourcePanel.svelte';

vi.mock('$lib/api', () => ({
	ApiError: class ApiError extends Error {},
}));

const old = { name: 'Atrox Old', species: 'Atrox', maturity: 'Old' };
const young = { name: 'Atrox Young', species: 'Atrox', maturity: 'Young' };
const daikiba = { display: 'Daikiba Mature', species: 'Daikiba', maturity: 'Mature' };

function panel(overrides: Partial<OverlayMobSourceState> = {}): OverlayMobSourceState {
	return {
		kind: 'mobSource',
		lines: [
			{ kind: 'reading', label: 'Reading now', creature: old, note: null },
			{ kind: 'backstop', label: 'Your backstop', creature: young, note: null },
		],
		canType: true,
		pinned: false,
		saving: false,
		...overrides,
	};
}

function mount(state = panel(), results: unknown[] = [daikiba]) {
	const props = {
		panelState: state,
		search: vi.fn(async () => results as never),
		onSelect: vi.fn(),
		onDone: vi.fn(),
	};
	render(MobSourcePanel, { props });
	return props;
}

async function type(text: string) {
	await fireEvent.input(screen.getByLabelText('Mob'), { target: { value: text } });
}

describe('where the mob comes from', () => {
	it('says each source in full, and clears the backstop', async () => {
		const props = mount();
		const lines = screen.getAllByTestId('mob-line');
		expect(lines.map((line) => line.dataset.kind)).toEqual(['reading', 'backstop']);
		expect(lines[0].textContent).toContain('Reading now');
		expect(lines[0].textContent).toContain('Atrox Old');
		await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
		expect(props.onSelect).toHaveBeenCalledWith({ kind: 'mobSource', action: 'clearBackstop' });
		expect(props.onDone).not.toHaveBeenCalled();
	});

	it('offers nothing to clear where no backstop is set, and no field where none can be typed', () => {
		mount(
			panel({
				lines: [{ kind: 'backstop', label: 'Your backstop', creature: null, note: null }],
				canType: false,
			}),
		);
		expect(screen.queryByRole('button', { name: 'Clear' })).toBeNull();
		expect(screen.queryByLabelText('Mob')).toBeNull();
	});
});

describe('typing a mob', () => {
	it('lists the catalogue mobs that match, and waits to be told what a pick is for', async () => {
		const props = mount();
		await type('dai');
		await waitFor(() =>
			expect(screen.getByRole('button', { name: 'Daikiba Mature' })).toBeTruthy(),
		);
		expect(props.search).toHaveBeenCalledWith('dai');
		await fireEvent.click(screen.getByRole('button', { name: 'Daikiba Mature' }));
		// Picked, not yet used: nothing is written.
		expect(props.onSelect).not.toHaveBeenCalled();
		expect(screen.getByTestId('mob-draft').textContent).toContain('Daikiba Mature');
		expect(screen.queryByTestId('mob-suggestions')).toBeNull();
	});

	it('pins the pick and is done, or keeps it as the backstop and stays', async () => {
		const pinned = mount();
		await type('dai');
		await fireEvent.click(await screen.findByRole('button', { name: 'Daikiba Mature' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Pin' }));
		expect(pinned.onSelect).toHaveBeenCalledWith({
			kind: 'mobSource',
			action: 'pin',
			species: 'Daikiba',
			maturity: 'Mature',
		});
		expect(pinned.onDone).toHaveBeenCalledTimes(1);
		document.body.replaceChildren();

		const kept = mount();
		await type('dai');
		await fireEvent.click(await screen.findByRole('button', { name: 'Daikiba Mature' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Use as backstop' }));
		expect(kept.onSelect).toHaveBeenCalledWith({
			kind: 'mobSource',
			action: 'backstop',
			species: 'Daikiba',
			maturity: 'Mature',
		});
		expect(kept.onDone).not.toHaveBeenCalled();
		// The field is ready for the next mob.
		expect((screen.getByLabelText('Mob') as HTMLInputElement).value).toBe('');
	});

	it('takes the top match on Enter: only a catalogue mob can be entered', async () => {
		mount();
		await type('dai');
		await screen.findByRole('button', { name: 'Daikiba Mature' });
		await fireEvent.keyDown(screen.getByLabelText('Mob'), { key: 'Enter' });
		expect(screen.getByTestId('mob-draft').textContent).toContain('Daikiba Mature');
	});

	it('offers only the pin while a mob is pinned, and says when nothing matches', async () => {
		mount(panel({ pinned: true }));
		await type('dai');
		await fireEvent.click(await screen.findByRole('button', { name: 'Daikiba Mature' }));
		expect(screen.getByRole('button', { name: 'Pin' })).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Use as backstop' })).toBeNull();
		document.body.replaceChildren();

		mount(panel(), []);
		await type('zzz');
		await waitFor(() => expect(screen.getByText('No matches')).toBeTruthy());
	});

	it('waits while a write is running', () => {
		mount(panel({ saving: true }));
		expect((screen.getByLabelText('Mob') as HTMLInputElement).disabled).toBe(true);
		expect((screen.getByRole('button', { name: 'Clear' }) as HTMLButtonElement).disabled).toBe(
			true,
		);
	});
});
