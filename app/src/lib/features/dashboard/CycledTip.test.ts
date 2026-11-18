// @vitest-environment happy-dom

import { render, screen, within } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import CycledTip from './CycledTip.svelte';

describe('CycledTip', () => {
	it('lists each item under its bucket with the total it sums to', () => {
		render(CycledTip, {
			lines: [
				{ bucket: 'weapon', label: 'Opalo', costPed: 10 },
				{ bucket: 'armour', label: 'Hyperion', costPed: 2 },
			],
			totalPed: 12,
		});
		const breakdown = screen.getByTestId('cycled-breakdown');
		expect(within(breakdown).getByText('Weapons')).toBeTruthy();
		expect(within(breakdown).getByText('Opalo')).toBeTruthy();
		expect(within(breakdown).getByText('Armour and plates')).toBeTruthy();
		expect(within(breakdown).getByText('83%')).toBeTruthy();
		expect(screen.getByText('12.00 PED')).toBeTruthy();
		// Armour is in, so the reminder that it arrives later is not.
		expect(screen.queryByText(/join once you record/)).toBeNull();
	});

	it('says armour arrives with a recording while none has', () => {
		render(CycledTip, {
			lines: [{ bucket: 'weapon', label: 'Opalo', costPed: 10 }],
			totalPed: 10,
		});
		expect(screen.getByText(/join once you record a repair/)).toBeTruthy();
	});

	it('carries what the figure leaves out', () => {
		render(CycledTip, {
			lines: [{ bucket: 'weapon', label: 'Opalo', costPed: 10 }],
			totalPed: 10,
			incomplete: '3 shots unpriced, up to +1.23 PED',
		});
		expect(screen.getByText('3 shots unpriced, up to +1.23 PED.')).toBeTruthy();
	});

	it('says where the costs come from while the account readings measure them', () => {
		render(CycledTip, {
			lines: [
				{ bucket: 'weapon', label: 'Opalo', costPed: 10 },
				{ bucket: 'armour', label: 'From your account', costPed: 1.13 },
			],
			totalPed: 11.13,
			note: 'Costs from your account up to 2:52 PM, tracked since',
			overheadPed: 0.11,
		});
		expect(screen.getByTestId('cycled-account-note').textContent).toBe(
			'Costs from your account up to 2:52 PM, tracked since.',
		);
		// The overhead beside Cycled, never in its total.
		expect(within(screen.getByTestId('cycled-overhead')).getByText('0.11 PED')).toBeTruthy();
		expect(screen.getByText('11.13 PED')).toBeTruthy();
	});

	it('needs no armour recording once the account measures the armour, even at nothing', () => {
		render(CycledTip, {
			lines: [
				{ bucket: 'weapon', label: 'Opalo', costPed: 10 },
				{ bucket: 'armour', label: 'From your account', costPed: 0 },
			],
			totalPed: 10,
			note: 'Costs from your account up to 2:52 PM, tracked since',
		});
		expect(screen.queryByText(/join once you record/)).toBeNull();
	});

	it('still asks for a recording when the account leaves the armour unmeasured', () => {
		render(CycledTip, {
			lines: [{ bucket: 'weapon', label: 'Opalo', costPed: 10 }],
			totalPed: 10,
			note: 'Costs from your account up to 2:52 PM, tracked since',
		});
		expect(screen.getByText(/join once you record a repair/)).toBeTruthy();
	});

	it('says nothing of the account, or of overhead, without them', () => {
		render(CycledTip, {
			lines: [{ bucket: 'weapon', label: 'Opalo', costPed: 10 }],
			totalPed: 10,
		});
		expect(screen.queryByTestId('cycled-account-note')).toBeNull();
		expect(screen.queryByTestId('cycled-overhead')).toBeNull();
	});

	it('reads as nothing spent before the first cost', () => {
		render(CycledTip, { lines: [], totalPed: 0 });
		expect(screen.getByText('Nothing spent yet.')).toBeTruthy();
		expect(screen.queryByTestId('cycled-breakdown')).toBeNull();
	});
});
