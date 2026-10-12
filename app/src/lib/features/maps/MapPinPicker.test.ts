// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { MapPin } from '$lib/api';
import MapPinPicker from './MapPinPicker.svelte';

const pin = (id: number, name: string, notes: string | null): MapPin => ({
	id,
	planet: 'Calypso',
	lon: 61_400 + id,
	lat: 75_800 + id,
	altitude: null,
	name,
	icon: id === 1 ? 'teleporter' : 'ore',
	kind: 'marker',
	radiusM: null,
	notes,
	sessionId: null,
	mapViewId: null,
	createdAt: 1,
	lastVisitedAt: null,
	cooldownUntil: null,
	pinConfigId: null,
	colour: null,
	cooldownColour: null,
	category: null,
	specialKind: null,
});

describe('MapPinPicker', () => {
	it('offers matching current-planet pins and centres the chosen result', async () => {
		const pins = [pin(1, 'Port Atlantis', 'South coast'), pin(2, 'North claim', 'Lysterium')];
		const onselect = vi.fn();
		render(MapPinPicker, { props: { pins, onselect } });

		await fireEvent.input(screen.getByRole('combobox'), { target: { value: 'north' } });
		const option = await waitFor(() => screen.getByRole('option', { name: /North claim/ }));
		// The matched word is brought forward in the suggestion.
		expect(option.querySelector('mark')?.textContent).toBe('North');
		await fireEvent.click(option);

		expect(onselect).toHaveBeenCalledWith(pins[1]);
		expect((screen.getByRole('combobox') as HTMLInputElement).value).toBe('');
	});
});
