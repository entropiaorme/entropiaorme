// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CreatureEntry, CreatureEntryCheck } from '$lib/api';

const api = vi.hoisted(() => ({
	getCreatureEntries: vi.fn(),
	checkCreatureEntry: vi.fn(),
	saveCreatureEntry: vi.fn(),
	removeCreatureEntry: vi.fn(),
}));
vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	...api,
}));

import MobsTab from './MobsTab.svelte';

const plain: CreatureEntry = {
	name: 'Gloomrat',
	species: 'Gloomrat',
	maturity: 'Old',
	anyNumber: false,
	source: 'player',
};
const numbered: CreatureEntry = {
	name: 'Survival Zombie',
	species: 'Zombie',
	maturity: '04',
	anyNumber: true,
	source: 'review',
};

const creature = { name: 'Survival Zombie 04', species: 'Zombie', maturity: '04' };

/** Type into the plate entry and let the check delay pass. */
async function type(text: string) {
	const input = screen.getByLabelText('Creature as its plate shows it');
	await fireEvent.input(input, { target: { value: text } });
	await vi.advanceTimersByTimeAsync(300);
}

async function mount(entries: CreatureEntry[], check?: CreatureEntryCheck) {
	api.getCreatureEntries.mockResolvedValue(entries);
	api.checkCreatureEntry.mockResolvedValue(check ?? { kind: 'refused', reason: 'unset' });
	render(MobsTab);
	await vi.advanceTimersByTimeAsync(0);
}

beforeEach(() => {
	vi.useFakeTimers();
	for (const fn of Object.values(api)) fn.mockReset();
});
afterEach(() => {
	vi.useRealTimers();
});

describe('the list of kept creatures', () => {
	it('says so when none are kept', async () => {
		await mount([]);
		await waitFor(() => expect(screen.getByText('No creatures kept yet.')).toBeTruthy());
	});

	it('shows each creature as its plate prints it, with what is worth saying about it', async () => {
		await mount([plain, numbered]);
		await waitFor(() => expect(screen.getByText('Gloomrat Old')).toBeTruthy());
		// Species equal to the name is not repeated.
		expect(screen.queryByText('Gloomrat')).toBeNull();
		expect(screen.getByText('Survival Zombie 04')).toBeTruthy();
		expect(screen.getByText('any number · Zombie · confirmed in a review')).toBeTruthy();
	});
});

describe('the line under the entry', () => {
	it('says what a ready entry would come to, at any number', async () => {
		await mount([], {
			kind: 'ready',
			name: 'Survival Zombie',
			maturity: '04',
			anyNumber: true,
		});
		await type('Survival Zombie 04');
		expect(screen.getByRole('status').textContent).toBe(
			'Survival Zombie, at any number (shown as 04)',
		);
	});

	it('says what is already known and what it is recorded as', async () => {
		await mount([], {
			kind: 'known',
			name: 'Atrox',
			maturity: 'Old',
			recordedAs: { ...creature, name: 'Atrox Old Alpha' },
		} as CreatureEntryCheck);
		await type('Atrox Old');
		expect(screen.getByRole('status').textContent).toBe(
			'Already known: Atrox Old, recorded as Atrox Old Alpha',
		);
	});

	it('leaves out the recorded name when it is the same one', async () => {
		await mount([], {
			kind: 'known',
			name: 'Atrox',
			maturity: 'Old',
			recordedAs: { ...creature, name: 'atrox old' },
		} as CreatureEntryCheck);
		await type('Atrox Old');
		expect(screen.getByRole('status').textContent).toBe('Already known: Atrox Old');
	});

	it('gives the reason when the entry is refused', async () => {
		await mount([], { kind: 'refused', reason: 'That is not a creature name.' });
		await type('???');
		expect(screen.getByRole('status').textContent).toBe('That is not a creature name.');
	});
});

describe('adding and editing', () => {
	it('keeps Add disabled until the entry checks out', async () => {
		await mount([], { kind: 'refused', reason: 'No.' });
		const add = screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement;
		expect(add.disabled).toBe(true);
		await type('???');
		expect(add.disabled).toBe(true);

		api.checkCreatureEntry.mockResolvedValue({
			kind: 'ready',
			name: 'Survival Zombie',
			maturity: '04',
			anyNumber: true,
		});
		await type('Survival Zombie 04');
		expect(add.disabled).toBe(false);
	});

	it('switches Add to Save and offers Cancel while a creature is edited', async () => {
		await mount([numbered]);
		await waitFor(() => expect(screen.getByText('Survival Zombie 04')).toBeTruthy());
		expect(screen.queryByRole('button', { name: 'Cancel' })).toBeNull();

		await fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
		expect(screen.getByRole('button', { name: 'Save' })).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Add' })).toBeNull();
		expect(
			(screen.getByLabelText('Creature as its plate shows it') as HTMLInputElement).value,
		).toBe('Survival Zombie 04');

		await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
		expect(screen.getByRole('button', { name: 'Add' })).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Cancel' })).toBeNull();
	});
});
