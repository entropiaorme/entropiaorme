// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { DetectedCreature, ReviewedMob } from '$lib/api';
import MobDetections from './MobDetections.svelte';
import type { MobDetectionModel } from './mobDetectionModel.svelte';

/** A mob on no list, its spelling still the screen's. */
function unlisted(overrides: Partial<ReviewedMob> = {}): ReviewedMob {
	return {
		mob: { kind: 'mob', species: 'Gloomrat', maturity: '', provisionalKey: 'GLOOMRAT L4 #0' },
		name: 'Gloomrat',
		killCount: 2,
		unsureCount: 0,
		exampleText: 'Gloomrot',
		currentName: 'Gloomrat',
		unlisted: true,
		reviewed: false,
		...overrides,
	};
}

/** A listed mob the screen named. */
function listed(overrides: Partial<ReviewedMob> = {}): ReviewedMob {
	return {
		mob: { kind: 'mob', species: 'Atrox', maturity: 'Old', provisionalKey: null },
		name: 'Atrox Old',
		killCount: 9,
		unsureCount: 4,
		exampleText: 'Atrox Old',
		currentName: 'Atrox Old',
		unlisted: false,
		reviewed: false,
		...overrides,
	};
}

function unnamed(overrides: Partial<ReviewedMob> = {}): ReviewedMob {
	return {
		mob: { kind: 'unnamed' },
		name: null,
		killCount: 7,
		unsureCount: 0,
		exampleText: null,
		currentName: null,
		unlisted: false,
		reviewed: false,
		...overrides,
	};
}

function model(mobs: ReviewedMob[], offer: DetectedCreature | null = null): MobDetectionModel {
	return {
		mobs,
		error: null,
		notice: null,
		busy: null,
		draft: (mob: ReviewedMob) => mob.name ?? '',
		offerFor: () => offer,
		acceptOffer: vi.fn(async () => {}),
		setDraft: vi.fn(),
		refresh: vi.fn(),
		name: vi.fn(async () => {}),
		restoreDeclared: vi.fn(async () => {}),
		dismissError: vi.fn(),
	};
}

describe('the mobs section', () => {
	it('is absent when nothing was detected', () => {
		render(MobDetections, { props: { model: model([]) } });
		expect(screen.queryByTestId('mob-detections')).toBeNull();
	});

	it('gives each mob one line: its name, its kills, and a quiet note of how sure', () => {
		render(MobDetections, { props: { model: model([listed(), unnamed()]) } });
		// Mob detection is an ordinary setting: its heading carries no mark.
		expect(screen.queryByText('In development')).toBeNull();
		const rows = screen.getAllByTestId('mob-detection');
		expect(rows.map((row) => row.dataset.line)).toEqual(['mob', 'unnamed']);
		expect(rows[0].textContent).toContain('Atrox Old');
		expect(rows[0].textContent).toContain('9 kills');
		expect(rows[0].textContent).toContain('4 less certain');
		// Nothing to act on until the player asks to correct it.
		expect(rows[0].querySelector('input')).toBeNull();
		expect(rows[0].textContent).not.toContain('Read as');
		expect(rows[0].textContent).not.toContain('Use declared mob');
	});

	it('corrects a mob by the one-action settle, a click away', async () => {
		const shown = listed();
		const m = model([shown]);
		render(MobDetections, { props: { model: m } });
		const correct = screen.getByRole('button', { name: 'Correct' });
		expect(correct.getAttribute('aria-expanded')).toBe('false');
		await fireEvent.click(correct);
		expect(correct.getAttribute('aria-expanded')).toBe('true');
		expect(screen.getByLabelText('Mob for 9 kills of Atrox Old')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm' }));
		expect(m.name).toHaveBeenCalledWith(shown);
		await fireEvent.click(screen.getByRole('button', { name: 'Use declared mob' }));
		expect(m.restoreDeclared).toHaveBeenCalledWith(shown);
	});

	it('asks about a mob on no list at once, with what the screen read', async () => {
		const shown = unlisted();
		const m = model([shown]);
		render(MobDetections, { props: { model: m } });
		expect(screen.getByText('not in the mob list')).toBeTruthy();
		expect(screen.getByText('Read as “Gloomrot”.')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Correct' })).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Keep as a new mob' }));
		expect(m.name).toHaveBeenCalledWith(shown);
	});

	it('says what a corrected line is recorded as now', () => {
		render(MobDetections, {
			props: { model: model([listed({ currentName: 'Daikiba Mature', reviewed: true })]) },
		});
		expect(screen.getByText('4 less certain; recorded as Daikiba Mature')).toBeTruthy();
	});

	it('offers the mob a loosely typed name may mean', async () => {
		const shown = unlisted();
		const m = model([shown], { name: 'Atrox Young', species: 'Atrox', maturity: 'Young' });
		render(MobDetections, { props: { model: m } });
		expect(screen.getByText(/Did you mean Atrox Young\?/)).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Use Atrox Young' }));
		expect(m.acceptOffer).toHaveBeenCalledWith(shown);
	});

	it('holds a typed name to the length the app keeps', () => {
		render(MobDetections, { props: { model: model([unlisted()]) } });
		expect(screen.getByRole('textbox').getAttribute('maxlength')).toBe('64');
	});
});

describe('the kills the screen could not name', () => {
	it('are one line, named in one action', async () => {
		const shown = unnamed();
		const m = model([shown]);
		render(MobDetections, { props: { model: m } });
		const row = screen.getByTestId('mob-detection');
		expect(row.textContent).toContain('Not named');
		expect(row.textContent).toContain('7 kills');
		expect(row.querySelector('input')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Name' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Record' }));
		expect(m.name).toHaveBeenCalledWith(shown);
		// Nothing to put back before they were named.
		expect(screen.queryByRole('button', { name: 'Put back' })).toBeNull();
	});

	it('can be put back once named', async () => {
		const shown = unnamed({ currentName: 'Atrox Old', reviewed: true });
		const m = model([shown]);
		render(MobDetections, { props: { model: m } });
		expect(screen.getByText('recorded as Atrox Old')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Name' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Put back' }));
		expect(m.restoreDeclared).toHaveBeenCalledWith(shown);
	});
});
