// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { StatId } from '$lib/statsRegistry';
import { getStatDef } from '$lib/statsRegistry';
import { NO_DATA } from '$lib/utils/format';

// The strip renders from props plus the overlayStats customisation state; the
// state module is the one side-effecting seam (the real module pulls in the
// Tauri preference plumbing), so it is replaced with a plain `{ current }`
// stub the tests assign before each render (vi.hoisted so the mock factory
// can reference it before top-level imports initialise). The stats registry
// is real: the pill assertions exercise the actual render functions.
const { overlayStats, statsScope, setStatsScope } = vi.hoisted(() => {
	type Pref = { id: string; enabled: boolean };
	return {
		overlayStats: { current: [] as Pref[] },
		statsScope: { current: 'instance' as 'instance' | 'lifetime' },
		setStatsScope: vi.fn(),
	};
});

// Only the state seam is stubbed; `scopedStats` stays real so the pill
// assertions exercise the actual instance/lifetime filter.
vi.mock('$lib/statsCustomisation.svelte', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/statsCustomisation.svelte')>()),
	overlayStats,
}));

// The scope module is the other Tauri-preference seam.
vi.mock('$lib/statsScope.svelte', () => ({
	statsScope,
	setStatsScope,
}));

import type { ProtectionOverview, TrackingLive, TrackingStatus } from '$lib/api';
import OverlayStrip from './OverlayStrip.svelte';

function liveData(overrides: Partial<TrackingLive> = {}): TrackingLive {
	return { status: 'idle', ...overrides };
}

function activeStatus(overrides: Partial<TrackingStatus> = {}): TrackingStatus {
	return { status: 'active', ...overrides };
}

const mixedProtection: ProtectionOverview = {
	sets: [
		{
			id: '1',
			kind: 'armour',
			name: 'UL armour',
			economyKind: 'unlimited',
			markupPercent: null,
			latestObservation: null,
			pendingReconciliations: 0,
			basisLocked: false,
			unsettledDamage: 0,
			unsettledDeflections: 0,
			unsettledSessions: 0,
		},
		{
			id: '2',
			kind: 'plates',
			name: 'L plates',
			economyKind: 'limited',
			markupPercent: 125,
			latestObservation: null,
			pendingReconciliations: 0,
			basisLocked: false,
			unsettledDamage: 0,
			unsettledDeflections: 0,
			unsettledSessions: 0,
		},
	],
	loadouts: [
		{
			id: 'loadout',
			name: 'Mixed',
			armour: { id: '1', name: 'UL armour', economyKind: 'unlimited', markupPercent: null },
			plates: { id: '2', name: 'L plates', economyKind: 'limited', markupPercent: 125 },
		},
	],
	activeLoadoutId: 'loadout',
	recentReconciliations: [],
	recentCostWindows: [],
};

beforeEach(() => {
	overlayStats.current = [];
});

describe('track / stop control', () => {
	it('renders TRACK when idle and forwards the click to onStart', async () => {
		const onStart = vi.fn();
		render(OverlayStrip, { props: { data: liveData(), onStart } });

		const button = screen.getByTitle('Start tracking');
		expect(button.textContent).toContain('TRACK');
		button.click();
		expect(onStart).toHaveBeenCalledTimes(1);
	});

	it('renders the stop control with the elapsed timer when active', async () => {
		const onStop = vi.fn();
		render(OverlayStrip, {
			props: { data: liveData({ status: 'active', elapsed: 3725 }), onStop },
		});

		const button = screen.getByTitle('Stop tracking');
		button.click();
		expect(onStop).toHaveBeenCalledTimes(1);
		expect(screen.queryByTitle('Start tracking')).toBeNull();
		// 3725s formats as h:mm:ss with zero-padded minutes and seconds.
		expect(screen.getByText('1:02:05')).toBeTruthy();
	});

	it('formats a sub-hour elapsed as m:ss', () => {
		render(OverlayStrip, { props: { data: liveData({ status: 'active', elapsed: 65 }) } });
		expect(screen.getByText('1:05')).toBeTruthy();
	});

	it('disables the control and shows a busy marker while toggling', () => {
		render(OverlayStrip, { props: { data: liveData(), toggling: true } });
		const button = screen.getByTitle('Start tracking') as HTMLButtonElement;
		expect(button.disabled).toBe(true);
		expect(button.textContent).toContain('...');
	});
});

describe('armour track decision prompt', () => {
	it('replaces the stop control during an active session and forwards the decision', () => {
		const onArmourTrackDecision = vi.fn();
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active' }),
				awaitingArmourTrackDecision: true,
				onArmourTrackDecision,
			},
		});

		expect(screen.getByText('Record armour costs?')).toBeTruthy();
		expect(screen.queryByTitle('Stop tracking')).toBeNull();

		screen.getByText('Record').click();
		expect(onArmourTrackDecision).toHaveBeenCalledWith('yes');
		screen.getByText('Later').click();
		expect(onArmourTrackDecision).toHaveBeenCalledWith('no');
	});

	it('does not interpose when the session is not active', () => {
		render(OverlayStrip, {
			props: { data: liveData(), awaitingArmourTrackDecision: true },
		});
		expect(screen.queryByText('Record protection?')).toBeNull();
		expect(screen.getByTitle('Start tracking')).toBeTruthy();
	});
});

describe('messages', () => {
	it('keeps tracking warnings out of the strip: the notice rail owns them', () => {
		const warning =
			'Harvest guardrail: Short Boards were looted while ChopChop Jr was equipped; costs are attributed to Timber Saw';
		render(OverlayStrip, {
			props: {
				data: liveData({
					status: 'active',
					warnings: [{ type: 'warning', description: warning, value: 0 }],
				}),
			},
		});
		expect(screen.queryByText(warning)).toBeNull();
		expect(screen.getByTitle('Stop tracking')).toBeTruthy();
	});
});

describe('session facets and declared mob', () => {
	it('disables the session chip during an unnamed active session: the session is session-grain', () => {
		render(OverlayStrip, {
			props: { data: liveData({ status: 'active', sessionName: null }), definitionEditable: false },
		});
		const chip = screen.getByTitle('The session is fixed while one runs') as HTMLButtonElement;
		expect(chip.disabled).toBe(true);
	});

	it('offers the session picker chip while idle', () => {
		const onDefinitionTrigger = vi.fn();
		render(OverlayStrip, {
			props: { data: liveData({ status: 'idle', sessionName: null }), onDefinitionTrigger },
		});
		const chip = screen.getByTitle('Pick the session for the next run');
		chip.click();
		expect(onDefinitionTrigger).toHaveBeenCalledTimes(1);
	});

	// The session is session-grain: editing it live could only
	// rewrite the whole session's history, so a running session offers no
	// control at all (not even a clear), and the record is where it gets
	// corrected.
	it('offers no session control at all during an active session', () => {
		render(OverlayStrip, {
			props: { data: liveData({ status: 'active', sessionName: 'ARIS Dailies' }) },
		});
		expect(screen.getByText('ARIS Dailies')).toBeTruthy();
		expect(screen.queryByText('Pick...')).toBeNull();
		expect(screen.queryByLabelText('Clear session')).toBeNull();
	});

	// A session always runs under a definition, so the chip offers no
	// clear: "nothing in particular" is picked from the menu (the
	// protected default), which keeps tracking from ever having nothing
	// to be an instance of.
	it('shows the selected session with no way to empty it', () => {
		render(OverlayStrip, {
			props: { data: liveData({ sessionName: 'ARIS Dailies', sessionDefinitionId: '1' }) },
		});

		expect(screen.getByText('ARIS Dailies')).toBeTruthy();
		expect(screen.queryByText('Pick...')).toBeNull();
		expect(screen.queryByLabelText('Clear session')).toBeNull();
	});

	it('edits the skill boost while idle', () => {
		const onBoostCommit = vi.fn();
		render(OverlayStrip, {
			props: { data: liveData({ status: 'idle' }), boostDraft: '50', onBoostCommit },
		});
		const input = screen.getByLabelText('Skill boost percent') as HTMLInputElement;
		expect(input.value).toBe('50');
	});

	// The boost's grain is finer than the session (it stamps each skill
	// gain), so a pill expiring mid-session is a recordable change and the
	// control stays live throughout.
	it('keeps the boost editable during an active session', () => {
		const onBoostCommit = vi.fn();
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active', skillBoostPercent: 50 }),
				boostDraft: '50',
				onBoostCommit,
			},
		});
		const input = screen.getByLabelText('Skill boost percent') as HTMLInputElement;
		expect(input.value).toBe('50');
		expect(input.disabled).toBe(false);
	});

	it('shows the mob input when no mob is declared', () => {
		render(OverlayStrip, { props: { data: liveData({ currentMob: null }) } });
		expect(screen.getByPlaceholderText('Mob...')).toBeTruthy();
	});

	it('hides the release control when no mob is declared', () => {
		render(OverlayStrip, { props: { data: liveData({ currentMob: null }) } });
		expect(screen.queryByLabelText('Release mob')).toBeNull();
	});

	it('shows the declared mob with a release control instead of the input', () => {
		const onReleaseMob = vi.fn();
		render(OverlayStrip, {
			props: { data: liveData({ currentMob: 'Atrox Young' }), onReleaseMob },
		});

		expect(screen.getByText('Atrox Young')).toBeTruthy();
		expect(screen.queryByPlaceholderText('Mob...')).toBeNull();

		screen.getByLabelText('Release mob').click();
		expect(onReleaseMob).toHaveBeenCalledTimes(1);
	});

	it('offers the mob control during an active session (declarations move mid-session)', () => {
		render(OverlayStrip, {
			props: { data: liveData({ status: 'active', currentMob: null }) },
		});
		expect(screen.getByPlaceholderText('Mob...')).toBeTruthy();
	});
});

describe('derived activity feedback', () => {
	it('names the activity the held tool implies', () => {
		render(OverlayStrip, {
			props: { data: liveData({ currentTool: 'ChopChop Jr', currentActivity: 'treecutting' }) },
		});
		expect(screen.getByTestId('activity-feedback').textContent?.trim()).toBe('Tree Cutting');
	});

	it('says nothing when no tool is known', () => {
		render(OverlayStrip, {
			props: { data: liveData({ currentTool: null, currentActivity: null }) },
		});
		expect(screen.queryByTestId('activity-feedback')).toBeNull();
	});

	it('shows a held healer as the sole item without backend healing telemetry', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({
					weaponAttribution: 'trifecta',
					currentTool: 'Restoration Chip 10',
					currentToolKind: 'healing',
					currentActivity: null,
					trifectaAttribution: {
						activePresetId: 'p1',
						presetName: 'Hunting Set',
						presets: [{ id: 'p1', name: 'Hunting Set' }],
						smallWeapon: null,
						bigWeapon: null,
						healTool: null,
					},
					healing: {
						toolName: 'Restoration Chip 10',
						state: 'cooldown',
						cooldownUntil: 20,
						effectUntil: null,
						activations: 2,
						directOutputs: 1,
						effectOutputs: 0,
						passiveOutputs: 3,
						unattributedOutputs: 0,
					},
				}),
			},
		});

		expect(screen.getByText('Restoration Chip 10')).toBeTruthy();
		expect(screen.queryByText('Hunting Set')).toBeNull();
		expect(screen.queryByTestId('activity-feedback')).toBeNull();
		expect(screen.queryByTestId('healing-state')).toBeNull();
		expect(screen.queryByText(/passive heal/i)).toBeNull();
		expect(screen.queryByText(/cooldown/i)).toBeNull();
	});
});

describe('protection declaration policy', () => {
	it('hides segment protection selectors when the definition records whole-session cost only', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ trackProtectionBySegment: false }),
				protection: mixedProtection,
			},
		});
		expect(screen.queryByTestId('protection-facet')).toBeNull();
	});

	it('removes armour controls altogether when armour-cost tracking is disabled', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active', trackProtectionCosts: false }),
				armourSessionId: 'session-1',
				protection: mixedProtection,
			},
		});

		expect(screen.queryByTestId('protection-facet')).toBeNull();
		expect(document.querySelector('[data-guide-anchor="overlay-armour-section"]')).toBeNull();
	});
});

describe('customisable stat pills', () => {
	it('renders only the enabled overlay stats, through the real registry render', () => {
		overlayStats.current = [
			{ id: 'net' as StatId, enabled: true },
			{ id: 'kills' as StatId, enabled: false },
		];
		const status = activeStatus({ cost: 10, returns: 12.5, kill_count: 7 });
		render(OverlayStrip, { props: { data: liveData({ status: 'active' }), status } });

		const netDef = getStatDef('net' as StatId);
		const killsDef = getStatDef('kills' as StatId);
		expect(netDef && screen.getByText(netDef.label)).toBeTruthy();
		expect(killsDef && screen.queryByText(killsDef.label)).toBeNull();
		// net = returns - cost, rendered by the registry's own formatter.
		const netRender = netDef ? netDef.render(status) : null;
		expect(netRender && screen.getByText(netRender.value)).toBeTruthy();
	});

	it('renders nothing when no overlay stat is enabled', () => {
		overlayStats.current = [{ id: 'net' as StatId, enabled: false }];
		const netDef = getStatDef('net' as StatId);
		render(OverlayStrip, { props: { data: liveData({ status: 'active' }) } });
		expect(netDef && screen.queryByText(netDef.label)).toBeNull();
	});
});

describe('trifecta selector', () => {
	const trifecta = {
		activePresetId: 'p1',
		presetName: 'Hunting Set',
		presets: [
			{ id: 'p1', name: 'Hunting Set' },
			{ id: 'p2', name: 'Mining Set' },
		],
		smallWeapon: null,
		bigWeapon: null,
		healTool: null,
	};

	it('renders the active preset name and forwards the trigger click with its anchor', () => {
		const onTrifectaTrigger = vi.fn();
		render(OverlayStrip, {
			props: {
				data: liveData({
					status: 'active',
					weaponAttribution: 'trifecta',
					trifectaAttribution: trifecta,
				}),
				onTrifectaTrigger,
			},
		});

		const trigger = screen.getByTitle('Hunting Set') as HTMLButtonElement;
		expect(trigger.getAttribute('aria-expanded')).toBe('false');
		trigger.click();
		expect(onTrifectaTrigger).toHaveBeenCalledWith(trigger);
	});

	it('reflects the open menu and saving state on the trigger', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ weaponAttribution: 'trifecta', trifectaAttribution: trifecta }),
				trifectaMenuOpen: true,
				trifectaSaving: true,
			},
		});

		const trigger = screen.getByTitle('Hunting Set') as HTMLButtonElement;
		expect(trigger.getAttribute('aria-expanded')).toBe('true');
		expect(trigger.disabled).toBe(true);
	});

	it('falls back to the current tool readout under hotbar attribution', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ weaponAttribution: 'hotbar', currentTool: 'Sollomate Opalo' }),
			},
		});
		expect(screen.getByText('Sollomate Opalo')).toBeTruthy();
		expect(screen.queryByTitle('Hunting Set')).toBeNull();
	});

	it('shows the guardrail alert in place of the tool readout on a mismatch', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({
					status: 'active',
					weaponAttribution: 'hotbar',
					currentTool: 'Terratech PH-4 (L)',
					harvestGuardrail: {
						expectedTool: 'Terratech PH-1 (L)',
						observedTool: 'Terratech PH-4 (L)',
						treeSize: 'short',
						atEpoch: 1_784_600_000,
					},
				}),
			},
		});
		const alert = screen.getByTestId('guardrail-alert');
		// The believed tool shows in red; the corrected attribution beneath.
		const believed = screen.getByText('Terratech PH-4 (L)');
		expect(believed.className).toContain('text-red-400');
		const recording = screen.getByText('Recording: Terratech PH-1 (L)');
		expect(recording.className).toContain('text-white/70');
		// The recorded tool must stay readable in full: no truncation.
		expect(recording.className).toContain('whitespace-nowrap');
		expect(recording.className).not.toContain('truncate');
		expect(alert.title).toBe(
			'Board output says Terratech PH-1 (L); hotbar shows Terratech PH-4 (L)',
		);
	});

	it('names the no-tool case in the guardrail alert', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({
					status: 'active',
					weaponAttribution: 'hotbar',
					harvestGuardrail: {
						expectedTool: 'Terratech PH-1 (L)',
						observedTool: null,
						treeSize: 'short',
						atEpoch: 1_784_600_000,
					},
				}),
			},
		});
		expect(screen.getByText('No tool').className).toContain('text-red-400');
		expect(screen.getByTestId('guardrail-alert').title).toBe(
			'Board output says Terratech PH-1 (L); hotbar shows no tool',
		);
	});
});

describe('armour cost control', () => {
	it('is disabled without a session id', () => {
		render(OverlayStrip, { props: { data: liveData() } });
		const button = screen.getByText('Cost') as HTMLButtonElement;
		expect(button.disabled).toBe(true);
	});

	it('toggles through the callback when a session id exists', () => {
		const onArmourCostToggle = vi.fn();
		render(OverlayStrip, {
			props: { data: liveData({ status: 'active' }), armourSessionId: 's1', onArmourCostToggle },
		});
		const button = screen.getByText('Cost') as HTMLButtonElement;
		expect(button.disabled).toBe(false);
		button.click();
		expect(onArmourCostToggle).toHaveBeenCalledTimes(1);
	});

	it('describes a mixed active loadout as a two-step protection flow', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active' }),
				armourSessionId: 's1',
				protection: mixedProtection,
			},
		});
		expect(screen.getByTitle('Record 2 armour costs')).toBeTruthy();
	});

	it('disables cost recording for an explicit no-protection loadout', () => {
		const protection: ProtectionOverview = {
			sets: [],
			loadouts: [{ id: 'none', name: 'No protection', armour: null, plates: null }],
			activeLoadoutId: 'none',
			recentReconciliations: [],
			recentCostWindows: [],
		};
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active' }),
				armourSessionId: 's1',
				protection,
			},
		});
		expect((screen.getByTitle('No armour cost to record') as HTMLButtonElement).disabled).toBe(
			true,
		);
	});

	it('stays available under whole-session attribution with no live selection', () => {
		// Whole-session attribution asks which setup was worn rather than reading
		// the live selection, so the control must not go dark just because
		// nothing is selected.
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active', trackProtectionBySegment: false }),
				armourSessionId: 's1',
				protection: { ...mixedProtection, activeLoadoutId: null },
			},
		});
		const button = screen.getByTitle('Record armour cost') as HTMLButtonElement;
		expect(button.disabled).toBe(false);
	});

	it('keeps the generic repair reading when the catalogue holds no setups', () => {
		// Nothing to choose between: the control offers the reading that needs no
		// composition rather than promising a flow it would refuse.
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active', trackProtectionBySegment: false }),
				armourSessionId: 's1',
			},
		});
		const button = screen.getByTitle('Record repair cost') as HTMLButtonElement;
		expect(button.disabled).toBe(false);
	});
});

describe('post-session bar', () => {
	const postSession = {
		data: liveData({ status: 'idle' }),
		lastSessionId: 's1',
	};

	it('replaces the active strip once a session has ended', () => {
		render(OverlayStrip, { props: postSession });
		expect(screen.getByText('Session ended')).toBeTruthy();
		expect(screen.queryByTitle('Start tracking')).toBeNull();
	});

	it('does not appear while idle with no finished session', () => {
		render(OverlayStrip, { props: { data: liveData() } });
		expect(screen.queryByText('Session ended')).toBeNull();
		expect(screen.getByTitle('Start tracking')).toBeTruthy();
	});

	it('renders the last-session cost and signed net', () => {
		render(OverlayStrip, {
			props: {
				...postSession,
				lastSessionStats: { cost: 25.5, returns: 27.75, pes: 1.2, net: 2.25 },
			},
		});
		expect(screen.getByText('25.50')).toBeTruthy();
		expect(screen.getByText('+2.25')).toBeTruthy();
	});

	it('renders a negative net without the plus sign', () => {
		render(OverlayStrip, {
			props: {
				...postSession,
				lastSessionStats: { cost: 25.5, returns: 20, pes: 1.2, net: -5.5 },
			},
		});
		expect(screen.getByText('-5.50')).toBeTruthy();
	});
});

describe('activities control', () => {
	function activities(overrides: Partial<NonNullable<TrackingLive['activities']>> = {}) {
		return {
			visible: true,
			adHocSegments: false,
			readyCount: 0,
			active: [],
			...overrides,
		};
	}

	it('is absent, not disabled, when the session has nothing to offer', () => {
		render(OverlayStrip, {
			props: { data: liveData({ status: 'active', activities: activities({ visible: false }) }) },
		});
		expect(screen.queryByTestId('activities-facet')).toBeNull();
	});

	it('is absent when the frame carries no readout at all', () => {
		render(OverlayStrip, { props: { data: liveData({ status: 'idle' }) } });
		expect(screen.queryByTestId('activities-facet')).toBeNull();
	});

	it('shows what the session will offer before tracking starts', () => {
		render(OverlayStrip, {
			props: { data: liveData({ status: 'idle', activities: activities({ readyCount: 3 }) }) },
		});
		expect(screen.getByTestId('activities-facet')).toBeTruthy();
		expect(screen.getByText('3 ready')).toBeTruthy();
	});

	it('shows how many rows a tap could start when nothing is recording', () => {
		render(OverlayStrip, {
			props: { data: liveData({ status: 'active', activities: activities({ readyCount: 3 }) }) },
		});
		expect(screen.getByText('3 ready')).toBeTruthy();
	});

	it('says nothing rather than promising a count it cannot honour', () => {
		render(OverlayStrip, {
			props: { data: liveData({ status: 'active', activities: activities() }) },
		});
		expect(screen.getByTestId('activities-facet').textContent).toContain(NO_DATA);
	});

	it('shows every standing activity as its own chip, whichever kind it is', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({
					status: 'active',
					activities: activities({
						active: [
							{
								key: 'quest:11',
								kind: 'quest',
								name: 'Daily: Carabok',
								questId: 11,
								manualHandIn: false,
								handInWaiting: false,
							},
							{
								key: 'segment:Boss lap',
								kind: 'segment',
								name: 'Boss lap',
								questId: null,
								manualHandIn: false,
								handInWaiting: false,
							},
						],
					}),
				}),
			},
		});
		expect(screen.getByText('Daily: Carabok')).toBeTruthy();
		expect(screen.getByText('Boss lap')).toBeTruthy();
	});

	it('opens the control from any of its chips', () => {
		const onActivitiesTrigger = vi.fn();
		render(OverlayStrip, {
			props: {
				data: liveData({
					status: 'active',
					activities: activities({
						active: [
							{
								key: 'quest:11',
								kind: 'quest',
								name: 'Daily: Carabok',
								questId: 11,
								manualHandIn: false,
								handInWaiting: false,
							},
						],
					}),
				}),
				onActivitiesTrigger,
			},
		});
		(screen.getByText('Daily: Carabok').closest('button') as HTMLButtonElement).click();
		expect(onActivitiesTrigger).toHaveBeenCalledTimes(1);
	});
});
