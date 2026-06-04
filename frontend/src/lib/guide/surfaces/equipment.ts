import { getDemoApi } from '../state.svelte';
import type { GuideSurface } from '../types';

/** Convenience: query an anchored element by its data-guide-anchor key. */
function anchor(key: string): HTMLElement | null {
	return document.querySelector(`[data-guide-anchor="${key}"]`);
}

/** Equipment-surface demoApi method names (declared here for documentation). */
type EquipmentDemoApi = {
	setActiveTab(tab: 'library' | 'trifecta' | 'hotbar'): void;
	expandRow(id: string): void;
	collapseRow(): void;
	openAddModal(type: 'weapon' | 'healing' | 'consumable'): void;
	closeAddModal(): void;
	setShowOptionalAttachments(value: boolean): void;
	setDemoHotbarEnabled(value: boolean): void;
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
		api.setDemoHotbarEnabled?.(true);
		api.collapseRow?.();
		api.closeAddModal?.();
		api.setShowOptionalAttachments?.(false);
	},
	steps: [
		{
			id: 'narrative-intro',
			prose: {
				title: 'Equipment',
				body: 'The Equipment tab sets up the loadout used for hunting cost tracking.',
				note: 'Note: Guide uses demo data.',
			},
		},
		{
			id: 'three-subtabs-overview',
			anchor: () => anchor('equipment-tabs'),
			prose: {
				title: 'Library, Trifecta, Hotbar',
				body: [
					{
						kind: 'p',
						text: 'There are two cost-tracking modes: Trifecta and Hotbar.',
					},
					{
						kind: 'p',
						text: 'The Library tab is where you add new equipment used by both.',
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
			id: 'trifecta-selectors',
			anchor: () => anchor('trifecta-selectors'),
			prose: {
				title: 'Trifecta mode',
				body: 'In Trifecta mode, you create presets of a small (tagger) weapon, a big (main) weapon, and your primary healing tool.',
				note: 'More healing tools, like restoration chip + FAP at the same time, coming soon.',
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<EquipmentDemoApi>;
				api.setDemoHotbarEnabled?.(false);
				api.setActiveTab?.('trifecta');
				await wait(500);
			},
			resetDemo() {
				const api = equipApi();
				api.setActiveTab?.('library');
				api.setDemoHotbarEnabled?.(true);
			},
		},
		{
			id: 'trifecta-damage-ranges',
			anchor: () => anchor('trifecta-chart'),
			prose: {
				title: 'Range-based attribution',
				body: "This chart shows each weapon's damage range. With non-overlapping ranges, every hit logged in chat.log is attributed to the weapon whose range contains it.",
				note: 'If small crits overlap big-weapon hits, the big weapon wins. This introduces some cost-attribution inaccuracy; use Hotbar mode to avoid it.',
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<EquipmentDemoApi>;
				api.setDemoHotbarEnabled?.(false);
				api.setActiveTab?.('trifecta');
				await wait(500);
			},
			resetDemo() {
				const api = equipApi();
				api.setActiveTab?.('library');
				api.setDemoHotbarEnabled?.(true);
			},
		},
		{
			id: 'hotbar-slot-list',
			anchor: () => anchor('hotbar-slot-list'),
			prose: {
				title: 'Hotbar mode',
				body: 'When Hotbar mode is active, keyboard presses in your number hotbar assign costs.',
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<EquipmentDemoApi>;
				api.setDemoHotbarEnabled?.(true);
				api.setActiveTab?.('hotbar');
				await wait(500);
			},
			resetDemo() {
				const api = equipApi();
				api.setActiveTab?.('library');
			},
		},
		{
			id: 'trifecta-default',
			prose: {
				title: 'Trifecta by default',
				body: 'Trifecta mode is on by default; you can switch to Hotbar mode in Settings.',
			},
		},
	],
};
