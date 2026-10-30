// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppSettings, EquipmentSummary } from '$lib/api/commands.gen';

vi.mock('$lib/api', () => ({
	declareEquipment: vi.fn(),
	getEquipmentLibrary: vi.fn(),
	getSettings: vi.fn(),
}));

import * as api from '$lib/api';
import { createEquipmentDeclare } from './equipmentDeclare.svelte';

const mocked = vi.mocked(api);

beforeEach(() => {
	vi.clearAllMocks();
});

describe('createEquipmentDeclare', () => {
	it('builds the picker over the library and the live hotbar', async () => {
		mocked.getEquipmentLibrary.mockResolvedValue([
			{ id: '1', name: 'Opalo', type: 'weapon' },
			{ id: '4', name: 'FAP', type: 'healing' },
		] as unknown as EquipmentSummary[]);
		mocked.getSettings.mockResolvedValue({
			hotbar: { '1': 1 },
			activeHotbarPreset: 1,
			hotbarPresets: [{ '1': 1 }, {}, {}],
			carriedWeaponIds: [],
		} as unknown as AppSettings);
		const model = createEquipmentDeclare({ onDeclared: vi.fn(), onError: vi.fn() });
		const state = await model.menuState(120, 'Opalo', 'FAP');
		expect(state.options.map((option) => option.name)).toEqual(['Opalo', 'FAP']);
		expect(state.inHand).toBe('Opalo');
		expect(state.healer).toBe('FAP');
	});

	it('declares, then re-reads the readout', async () => {
		mocked.declareEquipment.mockResolvedValue({ name: 'FAP', kind: 'healing' });
		const onDeclared = vi.fn();
		const model = createEquipmentDeclare({ onDeclared, onError: vi.fn() });
		await model.declare(4);
		expect(mocked.declareEquipment).toHaveBeenCalledWith(4);
		expect(onDeclared).toHaveBeenCalledOnce();
		expect(model.declaring).toBe(false);
	});

	it('reports a refused declaration and does not re-read', async () => {
		mocked.declareEquipment.mockRejectedValue(new Error('no tracking session is running'));
		const onDeclared = vi.fn();
		const onError = vi.fn();
		const model = createEquipmentDeclare({ onDeclared, onError });
		await model.declare(4);
		expect(onError).toHaveBeenCalledWith('no tracking session is running');
		expect(onDeclared).not.toHaveBeenCalled();
	});
});
