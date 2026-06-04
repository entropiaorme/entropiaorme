import { getDemoApi } from '../state.svelte';
import type { GuideSurface } from '../types';

/** Character-surface demoApi method names (declared here for documentation). */
type CharacterDemoApi = {
	setMainTab(tab: 'stats' | 'prospect' | 'optimizer' | 'codex'): void;
	setStatsSubTab(tab: 'attributes' | 'skills' | 'professions'): void;
	setFakeScannerVisible(visible: boolean): void;
	setProspectSeed(seed: boolean): void;
	setOptimizerSeed(seed: boolean): void;
	setCodexSeed(seed: boolean): void;
};

function characterApi(): Partial<CharacterDemoApi> {
	return getDemoApi('character') as Partial<CharacterDemoApi>;
}

/**
 * Animated illustration of the dock-skills-bottom-right workflow.
 * Monitor + window dragging to the bottom-right corner + camera-flash on the docked corner.
 * 3s loop via SMIL; no runtime cost beyond the SVG render.
 */
const skillScannerWorkflowSvg = `
<svg width="260" height="150" viewBox="0 0 260 150" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
	<rect x="30" y="14" width="200" height="120" rx="6" fill="rgb(40, 50, 65)" />
	<rect x="36" y="20" width="188" height="108" rx="2" fill="rgb(18, 25, 36)" />
	<rect x="114" y="134" width="32" height="6" fill="rgb(70, 88, 108)" />
	<rect x="92" y="140" width="76" height="4" rx="1" fill="rgb(70, 88, 108)" />
	<g>
		<rect width="62" height="9" rx="1.5" fill="rgb(99, 179, 237)" />
		<rect y="9" width="62" height="32" rx="1.5" fill="rgb(48, 60, 78)" stroke="rgb(99, 179, 237)" stroke-width="0.8" />
		<rect x="6" y="15" width="30" height="2.5" rx="0.5" fill="rgb(140, 160, 185)" />
		<rect x="6" y="21" width="48" height="1.8" rx="0.5" fill="rgb(85, 100, 122)" />
		<rect x="6" y="26" width="42" height="1.8" rx="0.5" fill="rgb(85, 100, 122)" />
		<rect x="6" y="31" width="46" height="1.8" rx="0.5" fill="rgb(85, 100, 122)" />
		<rect x="6" y="36" width="38" height="1.8" rx="0.5" fill="rgb(85, 100, 122)" />
		<animateTransform attributeName="transform" type="translate"
			values="70 42; 160 86; 160 86"
			keyTimes="0; 0.33; 1"
			calcMode="spline"
			keySplines="0.4 0 0.2 1; 0 0 1 1"
			dur="3s" repeatCount="indefinite" />
	</g>
	<rect x="160" y="86" width="64" height="41" rx="1.5" fill="white" opacity="0">
		<animate attributeName="opacity"
			values="0; 0; 0.85; 0; 0"
			keyTimes="0; 0.37; 0.43; 0.52; 1"
			dur="3s" repeatCount="indefinite" />
	</rect>
</svg>`.trim();

export const characterSurface: GuideSurface = {
	id: 'character',
	title: 'Character',
	beforeStart(demoApi) {
		const api = demoApi as Partial<CharacterDemoApi>;
		api.setMainTab?.('stats');
		api.setStatsSubTab?.('attributes');
		api.setFakeScannerVisible?.(false);
	},
	steps: [
		{
			id: 'narrative-intro',
			prose: {
				title: 'Character',
				body: [
					{ kind: 'p', text: 'The Character tab allows you to:' },
					{
						kind: 'ul',
						items: [
							'Scan your character skills.',
							'Enable skills tracking based on your scanned skills.',
							'Study skilling goals, optimisations, and codex reward recommendations.',
						],
					},
				],
				note: 'Note: Guide uses demo data.',
			},
		},
		{
			id: 'skill-scanner-spawn',
			anchor: () =>
				document.querySelector<HTMLElement>('[data-guide-anchor="character-scanner-spawn"]'),
			prose: {
				title: 'Skill scanner',
				body: [
					{
						kind: 'p',
						text: 'The skill scanner detects your EU window, and performs OCR in pre-selected coordinates.',
					},
					{
						kind: 'p',
						text: 'Dock your skills tab at the bottom right of your EU window, and follow the skill scanner overlay steps.',
					},
					{ kind: 'svg', svg: skillScannerWorkflowSvg },
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<CharacterDemoApi>;
				api.setMainTab?.('stats');
				api.setStatsSubTab?.('attributes');
				api.setFakeScannerVisible?.(true);
				await wait(500);
			},
			resetDemo() {
				characterApi().setFakeScannerVisible?.(false);
			},
		},
		{
			id: 'skill-progression-columns',
			placement: 'top',
			// Synthetic rect spanning the Anchor / Gain / Level column headers + their data cells.
			// Single-element anchors can't span an inner column range of a table; the rect-return
			// path on GuideStep.anchor lets us composite from the relevant <th>s and tbody.
			anchor: () => {
				const table = document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-skills-table"]',
				);
				if (!table) return null;
				const headerCells = table.querySelectorAll<HTMLElement>('thead th');
				if (headerCells.length < 4) return null;
				const anchorTh = headerCells[1];
				const levelTh = headerCells[3];
				const tbody = table.querySelector<HTMLElement>('tbody');
				if (!anchorTh || !levelTh || !tbody) return null;
				const a = anchorTh.getBoundingClientRect();
				const l = levelTh.getBoundingClientRect();
				const b = tbody.getBoundingClientRect();
				return new DOMRect(a.left, a.top, l.right - a.left, b.bottom - a.top);
			},
			prose: {
				title: 'Skill progression',
				body: [
					{ kind: 'p', text: 'Your skill stats grow automatically as you track sessions:' },
					{
						kind: 'ul',
						items: [
							'Anchor: your stats at your last skills scan.',
							'Gain: your tracked skill gains.',
							"Level: Anchor + Gain, the app's understanding of your current level.",
						],
					},
					{
						kind: 'p',
						text: "It's recommended to re-scan your skills every once in a while to refresh the anchor point.",
					},
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<CharacterDemoApi>;
				api.setMainTab?.('stats');
				api.setStatsSubTab?.('skills');
				await wait(300);
				// Push the table into the lower half of the viewport so the top-placed prose card
				// has clearance above the header row.
				const table = document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-skills-table"]',
				);
				if (table) {
					const rect = table.getBoundingClientRect();
					const desiredTop = 340;
					if (rect.top < desiredTop) {
						window.scrollBy({ top: rect.top - desiredTop, behavior: 'smooth' });
						await wait(400);
					}
				}
			},
			resetDemo() {
				characterApi().setStatsSubTab?.('attributes');
				window.scrollTo({ top: 0, behavior: 'smooth' });
			},
		},
		{
			id: 'skill-pes-column',
			// Synthetic rect spanning the PES column header + its tbody column extent.
			anchor: () => {
				const table = document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-skills-table"]',
				);
				if (!table) return null;
				const headerCells = table.querySelectorAll<HTMLElement>('thead th');
				if (headerCells.length < 6) return null;
				const pesTh = headerCells[5];
				const tbody = table.querySelector<HTMLElement>('tbody');
				if (!pesTh || !tbody) return null;
				const h = pesTh.getBoundingClientRect();
				const b = tbody.getBoundingClientRect();
				return new DOMRect(h.left, h.top, h.width, b.bottom - h.top);
			},
			prose: {
				title: 'PES',
				body: [
					{
						kind: 'p',
						text: [
							{
								text: 'Here you can see the value of your skills, in PES (PED, but for skills). See our article ',
							},
							{ text: '"What is PES?"', href: 'https://entropiaorme.com/articles/what-is-pes' },
							{ text: ' for the rationale behind separating skills from PED.' },
						],
					},
					{ kind: 'p', text: 'The app uses PES as the denomination for skills value throughout.' },
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<CharacterDemoApi>;
				api.setMainTab?.('stats');
				api.setStatsSubTab?.('skills');
				await wait(500);
			},
			resetDemo() {
				characterApi().setStatsSubTab?.('attributes');
			},
		},
		{
			id: 'prospect-overview',
			anchor: () =>
				document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-prospect-result-tiles"]',
				),
			prose: {
				title: 'Prospect',
				body: [
					{
						kind: 'p',
						text: 'The Prospect page projects your recorded sessions forward: how much cycle and how long to reach a profession level goal, at your current activity.',
					},
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<CharacterDemoApi>;
				api.setMainTab?.('prospect');
				api.setProspectSeed?.(true);
				await wait(500);
			},
			resetDemo() {
				const api = characterApi();
				api.setProspectSeed?.(false);
				api.setMainTab?.('stats');
				api.setStatsSubTab?.('attributes');
			},
		},
		{
			id: 'prospect-knobs',
			// Combined rect spanning the form section: profession row + slice-type segmented control + 4-col grid.
			anchor: () => {
				const first = document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-prospect-knob-first"]',
				);
				const last = document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-prospect-knob-last"]',
				);
				if (!first || !last) return null;
				const a = first.getBoundingClientRect();
				const b = last.getBoundingClientRect();
				const left = Math.min(a.left, b.left);
				const right = Math.max(a.right, b.right);
				return new DOMRect(left, a.top, right - left, b.bottom - a.top);
			},
			prose: {
				title: 'Knobs',
				body: [
					{ kind: 'p', text: 'Choose your:' },
					{
						kind: 'ul',
						items: [
							'Target profession.',
							'Activity type (global, tag, mob, or weapon).',
							'Session group within that activity.',
							'Target level.',
							'Average markup you expect from loot in that activity.',
						],
					},
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<CharacterDemoApi>;
				api.setMainTab?.('prospect');
				api.setProspectSeed?.(true);
				await wait(500);
			},
			resetDemo() {
				const api = characterApi();
				api.setProspectSeed?.(false);
				api.setMainTab?.('stats');
				api.setStatsSubTab?.('attributes');
			},
		},
		{
			id: 'optimizer-overview',
			anchor: () =>
				document.querySelector<HTMLElement>('[data-guide-anchor="character-optimizer-area"]'),
			placement: 'top-right',
			prose: {
				title: 'Optimiser',
				body: [
					{
						kind: 'p',
						text: 'Optimiser works as a traditional chip-in optimiser, using the official wiki chip-in tool data.',
					},
					{
						kind: 'p',
						text: 'For HP, it shows which skills increase your HP with the least amount of PES put into them.',
					},
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<CharacterDemoApi>;
				api.setMainTab?.('optimizer');
				api.setOptimizerSeed?.(true);
				await wait(500);
			},
			resetDemo() {
				const api = characterApi();
				api.setOptimizerSeed?.(false);
				api.setMainTab?.('stats');
				api.setStatsSubTab?.('attributes');
			},
		},
		{
			id: 'codex-overview',
			// Two cutout holes: small one around the profession dropdown (top bar) + big one
			// around the codex reward claim panel (right side). The even-odd-fill cutout SVG
			// path stitches both holes together visually under the same dim layer.
			anchor: () => {
				const select = document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-codex-profession-select"]',
				);
				const claim = document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-codex-recommendation"]',
				);
				const rects: DOMRect[] = [];
				if (select) rects.push(select.getBoundingClientRect());
				if (claim) rects.push(claim.getBoundingClientRect());
				return rects.length > 0 ? rects : null;
			},
			placement: 'bottom-left',
			placementAnchor: () =>
				document.querySelector<HTMLElement>(
					'[data-guide-anchor="character-codex-mobs-list-placement"]',
				),
			prose: {
				title: 'Codex',
				body: [
					{
						kind: 'p',
						text: 'Calibrate your codex rank per mob, then pick a profession to prioritise.',
					},
					{ kind: 'p', text: 'The reward options rank by contribution to that profession.' },
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<CharacterDemoApi>;
				api.setMainTab?.('codex');
				api.setCodexSeed?.(true);
				await wait(500);
			},
			resetDemo() {
				const api = characterApi();
				api.setCodexSeed?.(false);
				api.setMainTab?.('stats');
				api.setStatsSubTab?.('attributes');
			},
		},
	],
};
