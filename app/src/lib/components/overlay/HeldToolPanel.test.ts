// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { OverlayHeldToolState } from '$lib/windows/overlayMenu';
import HeldToolPanel from './HeldToolPanel.svelte';

function panel(overrides: Partial<OverlayHeldToolState> = {}): OverlayHeldToolState {
	return { kind: 'heldTool', lines: [], setup: null, actions: [], declaring: false, ...overrides };
}

describe('the tool in hand in full', () => {
	it('says the whole name, the set-up, and what a use is priced at', () => {
		render(HeldToolPanel, {
			props: {
				panelState: panel({
					setup: {
						name: 'ArMatrix LR-35 (L) with a very long modification name',
						fitted: 1,
						fittedText: '1 attachment fitted',
						rows: [{ label: 'Amplifier', value: 'Omegaton A104' }],
						cost: '12.50 PEC',
						per: 'per shot',
						tt: '11.90 PEC at TT',
					},
				}),
				onSelect: vi.fn(),
			},
		});
		expect(screen.getByTestId('held-tool-name').textContent).toBe(
			'ArMatrix LR-35 (L) with a very long modification name',
		);
		const setup = screen.getByTestId('held-tool-setup');
		expect(setup.textContent).toContain('Amplifier');
		expect(setup.textContent).toContain('Omegaton A104');
		const cost = screen.getByTestId('held-tool-cost');
		expect(cost.textContent).toContain('12.50 PEC');
		expect(cost.textContent).toContain('per shot');
		expect(cost.textContent).toContain('11.90 PEC at TT');
	});

	it('says what the reading says, and takes an entry the player picks where the screen cannot say', async () => {
		const onSelect = vi.fn();
		render(HeldToolPanel, {
			props: {
				panelState: panel({
					lines: ['Rifle is turned off in your equipment.'],
					actions: [
						{
							kind: 'declare',
							equipmentId: 7,
							name: 'Rifle',
							label: 'Use',
							accessibleName: 'Use Rifle',
						},
						{ kind: 'choose', label: 'Choose a tool' },
					],
				}),
				onSelect,
			},
		});
		expect(screen.getByTestId('held-tool-line').textContent).toBe(
			'Rifle is turned off in your equipment.',
		);
		await fireEvent.click(screen.getByRole('button', { name: 'Use Rifle' }));
		expect(onSelect).toHaveBeenLastCalledWith({
			kind: 'heldTool',
			action: 'declare',
			equipmentId: 7,
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Choose a tool' }));
		expect(onSelect).toHaveBeenLastCalledWith({ kind: 'heldTool', action: 'choose' });
	});

	it('waits while a declaration is being saved', () => {
		render(HeldToolPanel, {
			props: {
				panelState: panel({
					actions: [{ kind: 'choose', label: 'Choose a tool' }],
					declaring: true,
				}),
				onSelect: vi.fn(),
			},
		});
		expect(
			(screen.getByRole('button', { name: 'Choose a tool' }) as HTMLButtonElement).disabled,
		).toBe(true);
	});
});
