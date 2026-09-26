// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({ updateSettings: vi.fn() }));
vi.mock('$lib/api', () => api);

import EffectsTab from './EffectsTab.svelte';

beforeEach(() => vi.clearAllMocks());

describe('persistent effects', () => {
	it('saves a named reload-speed source through the typed settings boundary', async () => {
		const onchange = vi.fn();
		const saved = {
			passiveEffectSources: [
				{
					id: 'ares-perfect',
					name: 'Ares Ring, Perfected',
					enabled: true,
					effects: [{ kind: 'reload_speed', magnitudePercent: 14 }],
				},
			],
			reloadSpeed: { declaredPercent: 14, effectivePercent: 14, itemLimitPercent: 15 },
		};
		api.updateSettings.mockResolvedValue(saved);
		render(EffectsTab, { props: { sources: [], onchange } });

		await fireEvent.click(screen.getByText('Add effect'));
		await fireEvent.input(screen.getByPlaceholderText('Ares Ring, Perfected'), {
			target: { value: 'Ares Ring, Perfected' },
		});
		await fireEvent.input(screen.getByRole('spinbutton'), { target: { value: '14' } });
		await fireEvent.click(screen.getByText('Save effects'));

		await waitFor(() =>
			expect(api.updateSettings).toHaveBeenCalledWith({
				passive_effect_sources: [
					expect.objectContaining({
						name: 'Ares Ring, Perfected',
						enabled: true,
						effects: [{ kind: 'reload_speed', magnitude_percent: 14 }],
					}),
				],
			}),
		);
		// The owner gets the saved settings: their reload speed reprices weapons.
		expect(onchange).toHaveBeenCalledWith(saved);
	});

	it('discloses when the declared reload speed passes the item limit', () => {
		render(EffectsTab, {
			props: {
				sources: [
					{
						id: 'a',
						name: 'Ares Ring, Perfected',
						enabled: true,
						effects: [{ kind: 'reload_speed', magnitudePercent: 14 }],
					},
					{
						id: 'b',
						name: 'Mayhem armour',
						enabled: true,
						effects: [{ kind: 'reload_speed', magnitudePercent: 10 }],
					},
				],
				reloadSpeed: { declaredPercent: 24, effectivePercent: 15, itemLimitPercent: 15 },
			},
		});
		expect(screen.getByTestId('reload-limit-note').textContent).toBe(
			'Equipped items add at most 15% reload speed, so 15% of the 24% declared is in effect.',
		);
	});

	it('stays quiet while the declaration is within the limit', () => {
		render(EffectsTab, {
			props: {
				sources: [],
				reloadSpeed: { declaredPercent: 14, effectivePercent: 14, itemLimitPercent: 15 },
			},
		});
		expect(screen.queryByTestId('reload-limit-note')).toBeNull();
	});

	it('does not save an unnamed source', async () => {
		render(EffectsTab, { props: { sources: [] } });
		await fireEvent.click(screen.getByText('Add effect'));

		const save = screen.getByText('Save effects') as HTMLButtonElement;
		expect(save.disabled).toBe(true);
		expect(screen.getByText(/Name every source/)).toBeTruthy();
		expect(api.updateSettings).not.toHaveBeenCalled();
	});

	it('does not save a named source at the combined reload-speed limit', async () => {
		// A total of -100% would leave no reload interval at all, so the
		// boundary itself is refused rather than merely everything past it.
		render(EffectsTab, { props: { sources: [] } });
		await fireEvent.click(screen.getByText('Add effect'));
		await fireEvent.input(screen.getByPlaceholderText('Ares Ring, Perfected'), {
			target: { value: 'Broken Ring' },
		});
		await fireEvent.input(screen.getByRole('spinbutton'), { target: { value: '-100' } });

		expect((screen.getByText('Save effects') as HTMLButtonElement).disabled).toBe(true);
		expect(api.updateSettings).not.toHaveBeenCalled();
	});
});
