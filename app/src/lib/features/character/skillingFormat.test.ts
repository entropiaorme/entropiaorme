import { describe, expect, it } from 'vitest';
import type { SkillingForecastSample } from '$lib/api/commands.gen';
import {
	formatGoal,
	formatHours,
	formatLevel,
	formatLift,
	formatPes,
	formatTargetGain,
	formatTargetValue,
	groupByReason,
	statusLabel,
	statusMessage,
	warningText,
} from './skillingFormat';

const PROFESSION = { kind: 'profession', name: 'Evader' } as const;
const HP = { kind: 'hp' } as const;

describe('target values', () => {
	it('reads profession levels to two places and HP as whole points', () => {
		expect(formatTargetValue(PROFESSION, 42.375)).toBe('42.38');
		expect(formatTargetValue(HP, 142.7)).toBe('142');
	});

	it('reads a goal inline', () => {
		expect(formatGoal(PROFESSION, 43)).toBe('Lv 43');
		expect(formatGoal(PROFESSION, 43.5)).toBe('Lv 43.50');
		expect(formatGoal(HP, 150)).toBe('150 HP');
	});

	it('signs target gains at the precision each unit needs', () => {
		expect(formatTargetGain(PROFESSION, 0.0421)).toBe('+0.042');
		expect(formatTargetGain(HP, 1.234)).toBe('+1.23');
	});
});

describe('figures', () => {
	it('reads play time at a useful grain', () => {
		expect(formatHours(0)).toBe('0h');
		expect(formatHours(0.001)).toBe('1m');
		expect(formatHours(0.5)).toBe('30m');
		expect(formatHours(3.25)).toBe('3.3h');
		expect(formatHours(1234.4)).toBe('1,234h');
	});

	it('reads PES and markup lifts', () => {
		expect(formatPes(29.24)).toBe('29.2');
		expect(formatPes(1234.6)).toBe('1,235');
		expect(formatLift(0.0421)).toBe('+4.2%');
		expect(formatLift(-0.013)).toBe('-1.3%');
		// A small lift keeps its digits rather than reading as zero.
		expect(formatLift(0.0003)).toBe('+0.03%');
	});

	it('reads skill levels to two grouped places', () => {
		expect(formatLevel(788.063)).toBe('788.06');
		expect(formatLevel(1380.021)).toBe('1,380.02');
	});

	it('groups left-out skills by their reason', () => {
		expect(
			groupByReason([
				{ name: 'Alertness', reason: 'not needed for this goal' },
				{ name: 'Scavenging', reason: 'not unlocked' },
				{ name: 'Anatomy', reason: 'not needed for this goal' },
			]),
		).toBe('not needed for this goal: Alertness, Anatomy; not unlocked: Scavenging');
	});
});

describe('statuses and caveats', () => {
	it('says why a session cannot answer', () => {
		expect(statusMessage('ready', 'A', 'B')).toBe('');
		expect(statusMessage('does_not_train', 'Tree Cutting', 'Evader')).toBe(
			'Nothing Tree Cutting has trained in your recorded sessions moves Evader.',
		);
		expect(statusMessage('out_of_range', 'Tree Cutting', 'Evader')).toContain(
			"Tree Cutting's skill mix",
		);
		expect(statusLabel('no_evidence')).toBe('No recorded cycling');
		expect(statusLabel('ready')).toBe('');
	});

	it('names the figure behind each sample caveat', () => {
		const sample: SkillingForecastSample = {
			sessions: 1,
			hours: 0.75,
			cycledPed: 32.4,
			lootTt: 30,
			pes: 1,
			realisedMarkup: null,
			markupLift: null,
		};
		expect(warningText('thin_sessions', sample)).toBe('only 1 session recorded');
		expect(warningText('thin_hours', sample)).toBe('only 45m of play recorded');
		expect(warningText('thin_cycling', sample)).toBe('only 32 PED cycled');
		expect(warningText('thin_sessions', { ...sample, sessions: 2 })).toBe(
			'only 2 sessions recorded',
		);
	});
});
