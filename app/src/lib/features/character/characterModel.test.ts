import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CalibrationStatus, ComputedCharacterStats } from '$lib/api/commands.gen';
import type { ProfessionLevel, SkillLevel } from '$lib/types/analytics';
import { createCharacterModel, PAGE_SIZE } from './characterModel.svelte';

vi.mock('$lib/api', () => ({
	getCalibrationStatus: vi.fn(),
	getCharacterStats: vi.fn(),
	getCharacterSkills: vi.fn(),
	getCharacterProfessions: vi.fn(),
	getCharacterProspectOptions: vi.fn(),
	showScanOverlay: vi.fn(),
	getProfessionOptimizer: vi.fn(),
	getHpOptimizer: vi.fn(),
	getProfessionPathOptimizer: vi.fn(),
	getCharacterProspect: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function skill(overrides: Partial<SkillLevel> = {}): SkillLevel {
	return {
		name: 'Rifle',
		category: 'Combat',
		level: 1200,
		anchorLevel: 1100,
		gainSinceAnchor: 100,
		rankName: 'Adept',
		ttValue: 24,
		isAttribute: false,
		...overrides,
	};
}

function profession(overrides: Partial<ProfessionLevel> = {}): ProfessionLevel {
	return {
		name: 'Laser Sniper (Hit)',
		level: 40,
		anchorLevel: 38.5,
		gainSinceAnchor: 1.5,
		category: 'Combat',
		...overrides,
	};
}

function calibration(): CalibrationStatus {
	return { calibrated: true, lastCalibration: '2026-07-01T10:00:00Z', stale: false };
}

function stats(): ComputedCharacterStats {
	return { hp: 92, topProfessions: [] };
}

function seedLiveMocks() {
	mocked.getCalibrationStatus.mockResolvedValue(calibration());
	mocked.getCharacterStats.mockResolvedValue(stats());
	mocked.getCharacterSkills.mockResolvedValue([skill()]);
	mocked.getCharacterProfessions.mockResolvedValue([profession()]);
	mocked.getCharacterProspectOptions.mockResolvedValue({ tags: [], mobs: [], weapons: [] });
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('loadCharacterData', () => {
	it('loads calibration, stats, skills, professions and prospect options', async () => {
		seedLiveMocks();
		const model = createCharacterModel();
		await model.loadCharacterData();

		expect(model.calibration.calibrated).toBe(true);
		expect(model.stats.hp).toBe(92);
		expect(model.skills.map((s) => s.name)).toEqual(['Rifle']);
		expect(model.professions.map((p) => p.name)).toEqual(['Laser Sniper (Hit)']);
		expect(model.prospect.options).toEqual({ tags: [], mobs: [], weapons: [] });
		expect(model.loading).toBe(false);
		expect(model.error).toBeNull();
	});

	it('surfaces a load failure and keeps the defaults', async () => {
		seedLiveMocks();
		mocked.getCharacterSkills.mockRejectedValue(new Error('backend unreachable'));
		const model = createCharacterModel();
		await model.loadCharacterData();

		expect(model.error).toBe('backend unreachable');
		expect(model.skills).toEqual([]);
		expect(model.calibration.calibrated).toBe(false);
		expect(model.loading).toBe(false);
	});

	it('clears a stale error on entry', async () => {
		seedLiveMocks();
		const model = createCharacterModel();
		model.error = 'stale failure';
		await model.loadCharacterData();
		expect(model.error).toBeNull();
	});
});

describe('skills split and tables', () => {
	it('splits attributes from regular skills and feeds only the latter to the table', async () => {
		seedLiveMocks();
		mocked.getCharacterSkills.mockResolvedValue([
			skill({ name: 'Rifle' }),
			skill({ name: 'Strength', isAttribute: true }),
		]);
		const model = createCharacterModel();
		await model.loadCharacterData();

		expect(model.attributes.map((s) => s.name)).toEqual(['Strength']);
		expect(model.regularSkills.map((s) => s.name)).toEqual(['Rifle']);
		expect(model.skillsTable.filtered.map((s) => s.name)).toEqual(['Rifle']);
	});

	it('sorts the skills by level descending by default with nulls last on other keys', async () => {
		seedLiveMocks();
		mocked.getCharacterSkills.mockResolvedValue([
			skill({ name: 'Low', level: 10 }),
			skill({ name: 'High', level: 900 }),
			skill({ name: 'Unanchored', level: 500, anchorLevel: null }),
		]);
		const model = createCharacterModel();
		await model.loadCharacterData();

		expect(model.skillsTable.sortKey).toBe('level');
		expect(model.skillsTable.sortDir).toBe('desc');
		expect(model.skillsTable.filtered.map((s) => s.name)).toEqual(['High', 'Unanchored', 'Low']);

		model.skillsTable.setSort('anchorLevel');
		expect(model.skillsTable.filtered.at(-1)?.name).toBe('Unanchored');
	});

	it('pages the skills twelve per page and filters by category', async () => {
		seedLiveMocks();
		mocked.getCharacterSkills.mockResolvedValue([
			...Array.from({ length: 13 }, (_, i) => skill({ name: `Skill ${i}`, level: i })),
			skill({ name: 'Sweat Gatherer', category: 'General', level: 999 }),
		]);
		const model = createCharacterModel();
		await model.loadCharacterData();

		expect(PAGE_SIZE).toBe(12);
		expect(model.skillsTable.totalPages).toBe(2);
		expect(model.skillsTable.pageRows).toHaveLength(12);
		expect(model.skillsTable.categories).toEqual(['Combat', 'General']);

		model.skillsTable.page = 1;
		model.skillsTable.category = 'General';
		expect(model.skillsTable.page).toBe(0);
		expect(model.skillsTable.filtered.map((s) => s.name)).toEqual(['Sweat Gatherer']);
	});

	it('sorts the professions by level descending and searches by name', async () => {
		seedLiveMocks();
		mocked.getCharacterProfessions.mockResolvedValue([
			profession({ name: 'Miner', level: 12 }),
			profession({ name: 'Sniper', level: 55 }),
		]);
		const model = createCharacterModel();
		await model.loadCharacterData();

		expect(model.professionsTable.filtered.map((p) => p.name)).toEqual(['Sniper', 'Miner']);
		model.professionsTable.search = 'min';
		expect(model.professionsTable.filtered.map((p) => p.name)).toEqual(['Miner']);
	});
});

describe('openScanOverlay', () => {
	it('surfaces a failure to open the overlay', async () => {
		mocked.showScanOverlay.mockRejectedValue(new Error('overlay unavailable'));
		const model = createCharacterModel();
		model.openScanOverlay();
		await vi.waitFor(() => {
			expect(model.error).toBe('overlay unavailable');
		});
	});

	it('clears a stale error on entry', () => {
		mocked.showScanOverlay.mockResolvedValue(undefined);
		const model = createCharacterModel();
		model.error = 'stale failure';
		model.openScanOverlay();
		expect(model.error).toBeNull();
	});
});

describe('shared error slot', () => {
	it('exposes a sub-model failure at page level and clears it on the next load', async () => {
		seedLiveMocks();
		mocked.getHpOptimizer.mockRejectedValue(new Error('no data'));
		const model = createCharacterModel();
		await model.optimizer.loadHpOptimizer();
		expect(model.error).toBe('no data');

		await model.loadCharacterData();
		expect(model.error).toBeNull();
	});
});
