import { getDemoApi } from '../state.svelte';
import type { GuideSurface } from '../types';

/** Convenience: query an anchored element by its data-guide-anchor key. */
function anchor(key: string): HTMLElement | null {
	return document.querySelector(`[data-guide-anchor="${key}"]`);
}

/** Equipment-surface demoApi method names (declared here for documentation). */
type EquipmentDemoApi = {
	setActiveTab(tab: 'library' | 'hotbar'): void;
	expandRow(id: string): void;
	collapseRow(): void;
	openAddModal(type: 'weapon' | 'healing' | 'consumable'): void;
	closeAddModal(): void;
};

function equipApi(): Partial<EquipmentDemoApi> {
	return getDemoApi('equipment') as Partial<EquipmentDemoApi>;
}

export const equipmentSurface: GuideSurface = {
	id: 'equipment',
	title: 'Equipment',
	beforeStart(demoApi) {
		const api = demoApi as Partial<EquipmentDemoApi>;
		api.setActiveTab?.('library');
		api.collapseRow?.();
		api.closeAddModal?.();
	},
	steps: [
		{
			id: 'narrative-intro',
			prose: {
				title: 'Equipment',
				body: 'The Equipment tab sets up the gear used for hunting cost tracking.',
				note: 'Note: Guide uses demo data.',
			},
		},
		{
			id: 'three-subtabs-overview',
			anchor: () => anchor('equipment-tabs'),
			prose: {
				title: 'Library and Hotbar',
				body: [
					{
						kind: 'p',
						text: 'The Library tab is where you add your equipment.',
					},
					{
						kind: 'p',
						text: 'The Hotbar tab says which of it you carry, and how costs follow the weapon in your hand.',
					},
				],
			},
		},
		{
			id: 'add-equipment-button',
			anchor: () => anchor('add-equipment-button'),
			prose: {
				title: 'Add Equipment',
				body: 'The in-game item catalogue is built in: search for weapons and their mods (amps, enhancers, etc.); cost-per-use is calculated automatically.',
			},
		},
		{
			id: 'expected-return',
			anchor: () => anchor('expected-return-2'),
			prose: {
				title: 'Expected Return',
				body: [
					{
						kind: 'p',
						text: 'Expand a weapon to see each component’s own Efficiency, its long-run expected offensive return, and the loot markup that would break even under the community model.',
					},
					{
						kind: 'p',
						text: 'Effective Efficiency translates limited-item markup into the Efficiency of an economically equivalent unlimited setup. Healing, armour, harvesting, and other unmodelled costs stay outside this estimate; the information buttons explain both boundaries.',
					},
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<EquipmentDemoApi>;
				api.setActiveTab?.('library');
				api.expandRow?.('2');
				await wait(500);
			},
			resetDemo() {
				equipApi().collapseRow?.();
			},
		},
		{
			id: 'hotbar-slot-list',
			anchor: () => anchor('hotbar-slot-list'),
			prose: {
				title: 'Your hotbar',
				body: 'Bind each slot to the item it holds in game. With the hotbar key listener on, a press tells the app which weapon is in hand, so each shot costs what that weapon costs.',
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<EquipmentDemoApi>;
				api.setActiveTab?.('hotbar');
				await wait(500);
			},
			resetDemo() {
				equipApi().setActiveTab?.('library');
			},
		},
		{
			id: 'carried-weapons',
			anchor: () => anchor('carried-weapons'),
			prose: {
				title: 'Weapons without a hotkey',
				body: 'Add a weapon you switch to from the inventory here, so its hits are still recognised when no hotbar press announced it.',
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<EquipmentDemoApi>;
				api.setActiveTab?.('hotbar');
				await wait(500);
			},
			resetDemo() {
				equipApi().setActiveTab?.('library');
			},
		},
		{
			id: 'damage-ranges',
			anchor: () => anchor('damage-ranges-chart'),
			prose: {
				title: 'Damage ranges',
				body: [
					{
						kind: 'p',
						text: 'Every hit is checked against these ranges. A hit only another carried weapon explains is recorded to that weapon, and the overlay asks you to confirm the switch.',
					},
					{
						kind: 'p',
						text: 'A weapon whose cast keeps dealing damage shows its ticks too, once its damage pattern is set in the weapon’s form. Ticks cost nothing and never count as shots.',
					},
				],
				note: 'Where two ranges overlap, only the hotbar tells the weapons apart. A hit there with neither in hand is kept unpriced until you assign it after the session.',
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<EquipmentDemoApi>;
				api.setActiveTab?.('hotbar');
				await wait(500);
			},
			resetDemo() {
				equipApi().setActiveTab?.('library');
			},
		},
		{
			id: 'damage-alone',
			prose: {
				title: 'Without the hotbar listener',
				body: 'With the listener off in Settings, the damage ranges alone attribute each shot: a hit only one weapon explains is priced to it, and the rest wait for you in the session record.',
			},
		},
	],
};
