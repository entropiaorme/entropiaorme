// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { DetectedCreature, MobDetectionGroup } from '$lib/api';
import MobDetections from './MobDetections.svelte';
import type { MobDetectionModel } from './mobDetectionModel.svelte';

function group(overrides: Partial<MobDetectionGroup> = {}): MobDetectionGroup {
	return {
		group: {
			grade: 'provisional',
			creature: { name: 'Gloomrat', species: '', maturity: '' },
			provisionalKey: 'GLOOMRAT L4 #0',
		},
		killCount: 2,
		exampleText: 'Gloomrot',
		currentName: 'Gloomrat',
		reviewed: false,
		...overrides,
	};
}

function model(
	groups: MobDetectionGroup[],
	offer: DetectedCreature | null = null,
): MobDetectionModel {
	return {
		groups,
		error: null,
		notice: null,
		busy: null,
		draft: (g: MobDetectionGroup) => g.group.creature.name,
		offerFor: () => offer,
		acceptOffer: vi.fn(async () => {}),
		setDraft: vi.fn(),
		refresh: vi.fn(),
		name: vi.fn(async () => {}),
		restoreDeclared: vi.fn(async () => {}),
		dismissError: vi.fn(),
	};
}

describe('the detected creatures section', () => {
	it('is absent when nothing was detected', () => {
		render(MobDetections, { props: { model: model([]) } });
		expect(screen.queryByTestId('mob-detections')).toBeNull();
	});

	it('shows each group with its kills, what was read and how sure, and settles it in one action', async () => {
		const shown = group();
		const m = model([shown]);
		render(MobDetections, { props: { model: m } });
		expect(screen.getByText('Gloomrat')).toBeTruthy();
		expect(screen.getByText('Not in the creature list')).toBeTruthy();
		expect(screen.getByText('2 kills')).toBeTruthy();
		expect(screen.getByText(/Read as “Gloomrot”; recorded as Gloomrat/)).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Keep as a new creature' }));
		expect(m.name).toHaveBeenCalledWith(shown);
		await fireEvent.click(screen.getByRole('button', { name: 'Use declared mob' }));
		expect(m.restoreDeclared).toHaveBeenCalledWith(shown);
	});

	it('says a suggestion left the kills on the declared mob', () => {
		render(MobDetections, {
			props: {
				model: model([
					group({
						group: {
							grade: 'latest_read',
							creature: { name: 'Old Atrox', species: 'Atrox', maturity: 'Old' },
							provisionalKey: null,
						},
						exampleText: 'Atrox Old',
						currentName: 'Young Atrox',
						killCount: 1,
					}),
				]),
			},
		});
		expect(screen.getByText(/recorded as Young Atrox, your declared mob/)).toBeTruthy();
		expect(screen.getByText('Suggested: the last creature read before the kill')).toBeTruthy();
	});

	it('offers the creature a loosely typed name may mean', async () => {
		const shown = group();
		const m = model([shown], { name: 'Young Atrox', species: 'Atrox', maturity: 'Young' });
		render(MobDetections, { props: { model: m } });
		expect(screen.getByText(/Did you mean Young Atrox\?/)).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Use Young Atrox' }));
		expect(m.acceptOffer).toHaveBeenCalledWith(shown);
	});

	it('holds a typed name to the length the app keeps', () => {
		render(MobDetections, { props: { model: model([group()]) } });
		expect(screen.getByRole('textbox').getAttribute('maxlength')).toBe('64');
	});
});
