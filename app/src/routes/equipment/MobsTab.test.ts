// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CreatureEntry, CreatureEntryCheck, MobLabel } from '$lib/api';

const api = vi.hoisted(() => ({
	getCreatureEntries: vi.fn(),
	checkCreatureEntry: vi.fn(),
	saveCreatureEntry: vi.fn(),
	removeCreatureEntry: vi.fn(),
	getMobLabels: vi.fn(),
	setMobLabel: vi.fn(),
	clearMobLabel: vi.fn(),
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
	const input = screen.getByLabelText('Mob as its plate shows it');
	await fireEvent.input(input, { target: { value: text } });
	await vi.advanceTimersByTimeAsync(300);
}

async function mount(
	entries: CreatureEntry[],
	check?: CreatureEntryCheck,
	labels: MobLabel[] = [],
) {
	api.getCreatureEntries.mockResolvedValue(entries);
	api.getMobLabels.mockResolvedValue(labels);
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

describe('the list of kept mobs', () => {
	it('says so when none are kept', async () => {
		await mount([]);
		await waitFor(() => expect(screen.getByText('No mobs kept yet.')).toBeTruthy());
	});

	it('shows each mob as its plate prints it, with what is worth saying about it', async () => {
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
		await mount([], { kind: 'refused', reason: 'That is not a mob name.' });
		await type('???');
		expect(screen.getByRole('status').textContent).toBe('That is not a mob name.');
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

	it('switches Add to Save and offers Cancel while a mob is edited', async () => {
		await mount([numbered]);
		await waitFor(() => expect(screen.getByText('Survival Zombie 04')).toBeTruthy());
		expect(screen.queryByRole('button', { name: 'Cancel' })).toBeNull();

		await fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
		expect(screen.getByRole('button', { name: 'Save' })).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Add' })).toBeNull();
		expect((screen.getByLabelText('Mob as its plate shows it') as HTMLInputElement).value).toBe(
			'Survival Zombie 04',
		);

		await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
		expect(screen.getByRole('button', { name: 'Add' })).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Cancel' })).toBeNull();
	});
});

describe('what mobs are shown as', () => {
	const printed: MobLabel = {
		species: 'Survival Zombie Dog',
		maturity: '4',
		printed: 'Survival Zombie Dog 4',
		label: 'Survival Zombie Dog 04',
		byPlayer: false,
	};
	const chosen: MobLabel = {
		species: 'Atrox',
		maturity: 'Old',
		printed: 'Atrox Old',
		label: 'Old Trox',
		byPlayer: true,
	};

	it('lists each label with the mob it stands for, and says which the plate gave', async () => {
		await mount([], undefined, [printed, chosen]);
		const rows = screen.getAllByTestId('mob-label');
		expect(rows).toHaveLength(2);
		expect(rows[0].textContent).toContain('Survival Zombie Dog 04');
		expect(rows[0].textContent).toContain('as its plate prints it');
		expect(rows[1].textContent).toContain('Old Trox');
		expect(rows[1].textContent).toContain('Atrox Old');
		expect(rows[1].textContent).not.toContain('as its plate prints it');
	});

	it('labels a mob typed as the game shows it, once both halves are typed', async () => {
		await mount([]);
		const set = screen.getByRole('button', { name: 'Set label' }) as HTMLButtonElement;
		expect(set.disabled).toBe(true);
		await fireEvent.input(screen.getByLabelText('Mob to label'), {
			target: { value: 'Atrox Old' },
		});
		expect(set.disabled).toBe(true);
		await fireEvent.input(screen.getByLabelText('Label'), { target: { value: 'Old Trox' } });
		expect(set.disabled).toBe(false);
		api.setMobLabel.mockResolvedValue([chosen]);
		await fireEvent.click(set);
		await vi.advanceTimersByTimeAsync(0);
		expect(api.setMobLabel).toHaveBeenCalledWith('Atrox Old', 'Old Trox');
		expect(screen.getByTestId('mob-label').textContent).toContain('Old Trox');
		expect((screen.getByLabelText('Mob to label') as HTMLInputElement).value).toBe('');
	});

	it('puts a label back in the entry to change it, and removes one', async () => {
		await mount([], undefined, [chosen]);
		await fireEvent.click(screen.getByRole('button', { name: 'Change the label of Atrox Old' }));
		expect((screen.getByLabelText('Mob to label') as HTMLInputElement).value).toBe('Atrox Old');
		expect((screen.getByLabelText('Label') as HTMLInputElement).value).toBe('Old Trox');
		api.clearMobLabel.mockResolvedValue([]);
		await fireEvent.click(screen.getByRole('button', { name: 'Remove the label of Atrox Old' }));
		await vi.advanceTimersByTimeAsync(0);
		expect(api.clearMobLabel).toHaveBeenCalledWith('Atrox', 'Old');
		expect(screen.queryByTestId('mob-label')).toBeNull();
	});

	it('says why a label was refused and keeps what was typed', async () => {
		await mount([]);
		await fireEvent.input(screen.getByLabelText('Mob to label'), {
			target: { value: 'Gloomrat Bruiser' },
		});
		await fireEvent.input(screen.getByLabelText('Label'), { target: { value: 'Bruiser' } });
		api.setMobLabel.mockRejectedValue(new Error('"Gloomrat Bruiser" is not a mob the app knows'));
		await fireEvent.click(screen.getByRole('button', { name: 'Set label' }));
		await vi.advanceTimersByTimeAsync(0);
		expect(screen.getByText(/is not a mob the app knows/)).toBeTruthy();
		expect((screen.getByLabelText('Label') as HTMLInputElement).value).toBe('Bruiser');
	});
});
