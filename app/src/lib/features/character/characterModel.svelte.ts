/**
 * Character-surface view model: calibration, stats, the skill and profession
 * tables, the shared data load, and the skilling-hub sub-model. The stats
 * surface reports into one page-level error slot; the skilling hub reports
 * into per-facet slots of its own. Presentation lives in the feature
 * components; they compose over this state.
 */

import {
	getCalibrationStatus,
	getCharacterProfessions,
	getCharacterSkills,
	getCharacterStats,
	showScanOverlay,
} from '$lib/api';
import type { ProfessionLevel, SkillLevel, StatProfession } from '$lib/types/analytics';
import { describeError } from '$lib/view/errorState';
import { createTableModel } from '$lib/view/tableModel.svelte';
import { createErrorSlot } from './errorSlot.svelte';
import { createSkillingModel } from './skillingModel.svelte';

export const PAGE_SIZE = 12;

export function createCharacterModel() {
	const errors = createErrorSlot();

	let calibration = $state({
		calibrated: false,
		lastCalibration: null as string | null,
		stale: true,
	});
	let stats = $state({ hp: 80, topProfessions: [] as StatProfession[] });
	let skills = $state([] as SkillLevel[]);
	let professions = $state([] as ProfessionLevel[]);
	let loading = $state(true);

	const skilling = createSkillingModel({
		professions: () => professions,
		hp: () => stats.hp,
	});

	// ── Split attributes from regular skills ──
	const attributes = $derived(skills.filter((s) => s.isAttribute));
	const regularSkills = $derived(skills.filter((s) => !s.isAttribute));

	const skillsTable = createTableModel<SkillLevel>({
		rows: () => regularSkills,
		pageSize: PAGE_SIZE,
		searchText: (s) => [s.name],
		categoryOf: (s) => s.category,
		initialSort: { key: 'level', dir: 'desc' },
	});

	const professionsTable = createTableModel<ProfessionLevel>({
		rows: () => professions,
		pageSize: PAGE_SIZE,
		searchText: (p) => [p.name],
		initialSort: { key: 'level', dir: 'desc' },
	});

	async function loadCharacterData() {
		errors.error = null;
		try {
			const [cal, st, sk, pr] = await Promise.all([
				getCalibrationStatus(),
				getCharacterStats(),
				getCharacterSkills(),
				getCharacterProfessions(),
			]);
			calibration = cal;
			stats = st;
			skills = sk;
			professions = pr;
		} catch (e) {
			errors.error = describeError(e, 'Failed to load character data');
		} finally {
			loading = false;
		}
	}

	function openScanOverlay() {
		errors.error = null;
		showScanOverlay().catch((e) => {
			errors.error = describeError(e, 'Failed to open the skill scanner');
		});
	}

	return {
		skilling,
		skillsTable,
		professionsTable,

		get error() {
			return errors.error;
		},
		set error(value: string | null) {
			errors.error = value;
		},
		get calibration() {
			return calibration;
		},
		get stats() {
			return stats;
		},
		get skills() {
			return skills;
		},
		get professions() {
			return professions;
		},
		get loading() {
			return loading;
		},
		get attributes() {
			return attributes;
		},
		get regularSkills() {
			return regularSkills;
		},

		loadCharacterData,
		openScanOverlay,
	};
}

export type CharacterModel = ReturnType<typeof createCharacterModel>;
