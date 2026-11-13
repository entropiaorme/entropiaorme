// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { StatId } from '$lib/statsRegistry';
import { getStatDef } from '$lib/statsRegistry';

// The readout renders from props plus the overlayStats customisation state; the
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

// Only the state seam is stubbed; `enabledStats` stays real so the pill
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

import type { TrackingLive, TrackingStatus } from '$lib/api';
import { createMobSourceModel } from '$lib/features/tracking/mobSourceModel.svelte';
import OverlayReadout from './OverlayReadout.svelte';

function liveData(overrides: Partial<TrackingLive> = {}): TrackingLive {
	return { status: 'idle', ...overrides };
}

/** The mob source with only a typed mob: the typed mob alone. */
function typedMob(name: string): NonNullable<TrackingLive['mobSource']> {
	return {
		readingScreen: false,
		pin: null,
		typed: { name, species: name, maturity: '' },
		reading: null,
		continuity: null,
	};
}

function activeStatus(overrides: Partial<TrackingStatus> = {}): TrackingStatus {
	return { status: 'active', ...overrides };
}

beforeEach(() => {
	overlayStats.current = [];
});

describe('track / stop control', () => {
	it('renders TRACK when idle and forwards the click to onStart', async () => {
		const onStart = vi.fn();
		render(OverlayReadout, { props: { data: liveData(), onStart } });

		const button = screen.getByTitle('Start tracking');
		expect(screen.getByText('Track')).toBeTruthy();
		button.click();
		expect(onStart).toHaveBeenCalledTimes(1);
	});

	it('renders the stop control with the elapsed timer when active', async () => {
		const onStop = vi.fn();
		render(OverlayReadout, {
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
		render(OverlayReadout, { props: { data: liveData({ status: 'active', elapsed: 65 }) } });
		expect(screen.getByText('1:05')).toBeTruthy();
	});

	it('disables the control and shows a busy marker while toggling', () => {
		render(OverlayReadout, { props: { data: liveData(), toggling: true } });
		const button = screen.getByTitle('Start tracking') as HTMLButtonElement;
		expect(button.disabled).toBe(true);
		expect(button.textContent).toContain('...');
	});
});

describe('messages', () => {
	it('keeps tracking warnings out of the strip: the notice rail owns them', () => {
		const warning =
			'Harvest guardrail: Short Boards were looted while ChopChop Jr was equipped; costs are attributed to Timber Saw';
		render(OverlayReadout, {
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
	it('offers no session control during an unnamed active session: the session is session-grain', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', sessionName: null }), definitionEditable: false },
		});
		expect(screen.getByTestId('definition-facet').querySelector('button')).toBeNull();
	});

	it('offers the session picker chip while idle', () => {
		const onDefinitionTrigger = vi.fn();
		render(OverlayReadout, {
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
		render(OverlayReadout, {
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
		render(OverlayReadout, {
			props: { data: liveData({ sessionName: 'ARIS Dailies', sessionDefinitionId: '1' }) },
		});

		expect(screen.getByText('ARIS Dailies')).toBeTruthy();
		expect(screen.queryByText('Pick...')).toBeNull();
		expect(screen.queryByLabelText('Clear session')).toBeNull();
	});

	// The boost is declared in Equipment, with the other effects in force.
	it('carries no skill boost control', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', skillBoostPercent: 50 }) },
		});
		expect(screen.queryByLabelText('Skill boost percent')).toBeNull();
	});

	it('shows the mob input when no mob is declared', () => {
		render(OverlayReadout, { props: { data: liveData({ mobSource: null }) } });
		expect(screen.getByPlaceholderText('Type a mob...')).toBeTruthy();
	});

	it('hides the release control when no mob is declared', () => {
		render(OverlayReadout, { props: { data: liveData({ mobSource: null }) } });
		expect(screen.queryByLabelText('Release mob')).toBeNull();
	});

	it('shows the declared mob with a release control instead of the input', () => {
		const onReleaseMob = vi.fn();
		render(OverlayReadout, {
			props: { data: liveData({ mobSource: typedMob('Atrox Young') }), onReleaseMob },
		});

		expect(screen.getByText('Atrox Young')).toBeTruthy();
		expect(screen.queryByPlaceholderText('Type a mob...')).toBeNull();

		screen.getByLabelText('Release mob').click();
		expect(onReleaseMob).toHaveBeenCalledTimes(1);
	});

	it('offers the mob control during an active session (declarations move mid-session)', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', mobSource: null }) },
		});
		expect(screen.getByPlaceholderText('Type a mob...')).toBeTruthy();
	});
});

describe('derived activity feedback', () => {
	it('names the activity the held tool implies', () => {
		render(OverlayReadout, {
			props: { data: liveData({ currentTool: 'ChopChop Jr', currentActivity: 'treecutting' }) },
		});
		expect(screen.getByTestId('activity-feedback').textContent?.trim()).toBe('Tree Cutting');
	});

	it('says nothing when no tool is known', () => {
		render(OverlayReadout, {
			props: { data: liveData({ currentTool: null, currentActivity: null }) },
		});
		expect(screen.queryByTestId('activity-feedback')).toBeNull();
	});

	it('shows a held healer as the sole item without backend healing telemetry', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({
					currentTool: 'Restoration Chip 10',
					currentToolKind: 'healing',
					currentActivity: null,
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
		expect(screen.queryByTestId('activity-feedback')).toBeNull();
		expect(screen.queryByTestId('healing-state')).toBeNull();
		expect(screen.queryByText(/passive heal/i)).toBeNull();
		expect(screen.queryByText(/cooldown/i)).toBeNull();
	});
});

describe('the held weapon damage cue', () => {
	const held = {
		status: 'active' as const,
		currentTool: 'Sollomate Opalo',
		currentToolKind: 'weapon' as const,
		currentActivity: 'hunting' as const,
		weaponDeclared: true,
		hotbarListenerActive: true,
	};

	it('says when recent hits land above the listed range', () => {
		render(OverlayReadout, { props: { data: liveData({ ...held, weaponHitCue: 'boosted' }) } });
		const cue = screen.getByTestId('weapon-hit-cue');
		expect(cue.textContent?.trim()).toBe('Boosted');
		expect(cue.getAttribute('title')).toContain('press the slot of the weapon you are firing');
		expect(screen.queryByTestId('activity-feedback')).toBeNull();
	});

	it('says when recent hits land below the listed range', () => {
		render(OverlayReadout, { props: { data: liveData({ ...held, weaponHitCue: 'reduced' }) } });
		expect(screen.getByTestId('weapon-hit-cue').textContent?.trim()).toBe('Reduced');
	});

	it('leaves the activity line alone while hits land inside the range', () => {
		render(OverlayReadout, { props: { data: liveData({ ...held, weaponHitCue: null }) } });
		expect(screen.queryByTestId('weapon-hit-cue')).toBeNull();
		expect(screen.getByTestId('activity-feedback').textContent?.trim()).toBe('Hunting');
	});

	it('gives way to the prompt to declare a weapon', () => {
		render(OverlayReadout, {
			props: { data: liveData({ ...held, weaponDeclared: false, weaponHitCue: 'boosted' }) },
		});
		expect(screen.queryByTestId('weapon-hit-cue')).toBeNull();
		expect(screen.getByTestId('weapon-undeclared-cue')).toBeTruthy();
	});
});

describe('the undeclared weapon cue', () => {
	const undeclared = {
		status: 'active' as const,
		currentTool: 'Sollomate Opalo',
		currentToolKind: 'weapon' as const,
		currentActivity: 'hunting' as const,
		weaponDeclared: false,
		hotbarListenerActive: true,
	};

	it('asks for a slot press while the shown weapon is only inferred', () => {
		render(OverlayReadout, { props: { data: liveData(undeclared) } });
		expect(screen.getByTestId('weapon-undeclared-cue').textContent?.trim()).toBe(
			'Press weapon slot',
		);
		expect(screen.getByText('Sollomate Opalo').getAttribute('title')).toBe(
			'Sollomate Opalo: inferred from damage, not declared',
		);
		expect(screen.queryByTestId('activity-feedback')).toBeNull();
	});

	it('asks once unpriced shots land with no weapon to show', () => {
		render(OverlayReadout, {
			props: { data: liveData({ ...undeclared, currentTool: null, unpricedShots: 4 }) },
		});
		expect(screen.getByTestId('weapon-undeclared-cue')).toBeTruthy();
	});

	it('stays quiet before anything is fired', () => {
		render(OverlayReadout, {
			props: { data: liveData({ ...undeclared, currentTool: null, unpricedShots: 0 }) },
		});
		expect(screen.queryByTestId('weapon-undeclared-cue')).toBeNull();
	});

	it('stays quiet once a weapon is declared', () => {
		render(OverlayReadout, { props: { data: liveData({ ...undeclared, weaponDeclared: true }) } });
		expect(screen.queryByTestId('weapon-undeclared-cue')).toBeNull();
		// The name in full, for when the line cuts it short; no doubt cast on it.
		expect(screen.getByText('Sollomate Opalo').getAttribute('title')).toBe('Sollomate Opalo');
		expect(screen.getByTestId('activity-feedback').textContent?.trim()).toBe('Hunting');
	});

	it('stays quiet when no press could be seen', () => {
		render(OverlayReadout, {
			props: { data: liveData({ ...undeclared, hotbarListenerActive: false }) },
		});
		expect(screen.queryByTestId('weapon-undeclared-cue')).toBeNull();
	});

	it('asks for a pick instead of a press on a keyless session', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({ ...undeclared, hotbarListenerActive: false, keylessInput: true }),
			},
		});
		expect(screen.getByTestId('weapon-undeclared-cue').textContent?.trim()).toBe('Pick weapon');
	});
});

describe('the equipment picker trigger', () => {
	it('opens the picker from the glyph while a session runs', async () => {
		const onEquipmentTrigger = vi.fn();
		render(OverlayReadout, {
			props: {
				data: liveData({ status: 'active', currentTool: 'Opalo', currentToolKind: 'weapon' }),
				onEquipmentTrigger,
			},
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Declare equipment in hand' }));
		expect(onEquipmentTrigger).toHaveBeenCalledOnce();
	});

	it('is only a glyph while idle, when there is no hand to declare into', () => {
		render(OverlayReadout, { props: { data: liveData({ status: 'idle' }) } });
		expect(screen.queryByRole('button', { name: 'Declare equipment in hand' })).toBeNull();
		expect(screen.getByTestId('equipment-icon')).toBeTruthy();
	});
});

describe('customisable stat pills', () => {
	it('renders only the enabled overlay stats, through the real registry render', () => {
		overlayStats.current = [
			{ id: 'net' as StatId, enabled: true },
			{ id: 'kills' as StatId, enabled: false },
		];
		const status = activeStatus({ cost: 10, returns: 12.5, kill_count: 7 });
		render(OverlayReadout, { props: { data: liveData({ status: 'active' }), status } });

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
		render(OverlayReadout, { props: { data: liveData({ status: 'active' }) } });
		expect(netDef && screen.queryByText(netDef.label)).toBeNull();
	});
});

describe('the weapon guardrail cue', () => {
	const cue = {
		hotbarTool: 'Sollomate Opalo',
		recordingTool: 'Korss H400 (L)',
		since: 1_784_600_000,
		shots: 3,
	};

	it('shows the current weapon while the evidence agrees', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', currentTool: 'Sollomate Opalo' }) },
		});
		expect(screen.getByText('Sollomate Opalo')).toBeTruthy();
		expect(screen.queryByTestId('weapon-guardrail-alert')).toBeNull();
	});

	it('shows the doubted hotbar weapon, what is recorded, and the two decisions', async () => {
		const onWeaponDecision = vi.fn();
		render(OverlayReadout, {
			props: {
				data: liveData({ status: 'active', currentTool: 'Sollomate Opalo', weaponGuardrail: cue }),
				onWeaponDecision,
			},
		});
		const alert = screen.getByTestId('weapon-guardrail-alert');
		expect(alert).toBeTruthy();
		// What is recorded reads in the alert itself; the declared weapon
		// gives way beside it, with its name in full on hover.
		expect(alert.textContent?.trim()).toBe('Korss H400 (L)');
		expect(screen.getByText('not Sollomate Opalo').getAttribute('title')).toBe('Sollomate Opalo');

		(screen.getByLabelText('Confirm Korss H400 (L)') as HTMLButtonElement).click();
		expect(onWeaponDecision).toHaveBeenLastCalledWith('confirm');
		(screen.getByLabelText('Keep Sollomate Opalo') as HTMLButtonElement).click();
		expect(onWeaponDecision).toHaveBeenLastCalledWith('keep');
	});

	it('holds both decisions while one is saving', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({ status: 'active', weaponGuardrail: cue }),
				decidingWeapon: true,
			},
		});
		expect((screen.getByLabelText('Confirm Korss H400 (L)') as HTMLButtonElement).disabled).toBe(
			true,
		);
		expect((screen.getByLabelText('Keep Sollomate Opalo') as HTMLButtonElement).disabled).toBe(
			true,
		);
	});

	it('marks a cost pill incomplete while shots are unpriced', () => {
		overlayStats.current = [{ id: 'cycled' as StatId, enabled: true }];
		const status = activeStatus({
			cost: 10,
			returns: 12.5,
			unpricedShots: 2,
			unpricedCostBound: 0.4,
		});
		render(OverlayReadout, { props: { data: liveData({ status: 'active' }), status } });
		const value = screen.getByText('10.00');
		expect(value.title).toBe('2 shots unpriced, up to +0.40 PED');
		expect(value.textContent).toContain('*');
	});
});

describe('the harvest guardrail cue', () => {
	it('shows the guardrail alert in place of the tool readout on a mismatch', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({
					status: 'active',
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
		// The corrected attribution reads in the alert; the believed tool
		// gives way beside it.
		expect(alert.textContent?.trim()).toBe('Terratech PH-1 (L)');
		expect(screen.getByText('not Terratech PH-4 (L)')).toBeTruthy();
		expect(alert.title).toBe(
			'Board output says Terratech PH-1 (L); hotbar shows Terratech PH-4 (L)',
		);
	});

	it('names the no-tool case in the guardrail alert', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({
					status: 'active',
					harvestGuardrail: {
						expectedTool: 'Terratech PH-1 (L)',
						observedTool: null,
						treeSize: 'short',
						atEpoch: 1_784_600_000,
					},
				}),
			},
		});
		expect(screen.getByText('not no tool')).toBeTruthy();
		expect(screen.getByTestId('guardrail-alert').title).toBe(
			'Board output says Terratech PH-1 (L); hotbar shows no tool',
		);
	});
});

// Armour is recorded when the player repairs, and spread back over the
// sessions it covers, so nothing about it is declared during play and the
// control never waits on a running session.
describe('armour cost control', () => {
	it('is available with no session running and toggles through the callback', () => {
		const onArmourCostToggle = vi.fn();
		render(OverlayReadout, { props: { data: liveData(), onArmourCostToggle } });
		const button = screen.getByLabelText('Armour cost') as HTMLButtonElement;
		expect(button.disabled).toBe(false);
		button.click();
		expect(onArmourCostToggle).toHaveBeenCalledTimes(1);
	});

	it('stays offered under a session type that records no hits', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', trackProtectionCosts: false }) },
		});
		expect(screen.getByLabelText('Armour cost')).toBeTruthy();
	});

	it('offers no armour selector and asks nothing when a session stops', () => {
		render(OverlayReadout, { props: { data: liveData({ status: 'active' }) } });
		expect(screen.queryByTestId('protection-facet')).toBeNull();
		expect(screen.getByTitle('Stop tracking')).toBeTruthy();
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
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', activities: activities({ visible: false }) }) },
		});
		expect(screen.queryByTestId('activities-facet')).toBeNull();
	});

	it('is absent when the frame carries no readout at all', () => {
		render(OverlayReadout, { props: { data: liveData({ status: 'idle' }) } });
		expect(screen.queryByTestId('activities-facet')).toBeNull();
	});

	it('shows what the session will offer before tracking starts', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'idle', activities: activities({ readyCount: 3 }) }) },
		});
		expect(screen.getByTestId('activities-facet')).toBeTruthy();
		expect(screen.getByText('3 ready')).toBeTruthy();
	});

	it('shows how many rows a tap could start when nothing is recording', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', activities: activities({ readyCount: 3 }) }) },
		});
		expect(screen.getByText('3 ready')).toBeTruthy();
	});

	it('says nothing rather than promising a count it cannot honour', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', activities: activities() }) },
		});
		expect(screen.getByTestId('activities-facet').textContent).toContain('No activity declared');
	});

	it('shows every standing activity as its own chip, whichever kind it is', () => {
		render(OverlayReadout, {
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
		render(OverlayReadout, {
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

describe('the tool in hand read off the screen', () => {
	const fallback = {
		status: 'active' as const,
		// What the damage last pointed to: never shown as held while the
		// screen is the source.
		currentTool: 'Sollomate Opalo',
		currentToolKind: 'weapon' as const,
		currentActivity: 'hunting' as const,
		weaponDeclared: false,
		hotbarListenerActive: true,
	};

	type Source = NonNullable<TrackingLive['toolSource']>;

	function source(held: Source['held'], overrides: Partial<Source> = {}): Source {
		return { readingScreen: true, notReading: null, manual: false, held, ...overrides };
	}

	const pistol = source({ kind: 'tool', name: 'Pistol', provisional: false, setup: null });

	it('shows what the screen shows, and the undeclared cue stands down', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({
					...fallback,
					toolSource: source({ kind: 'uncertain', why: { kind: 'unreadable' } }),
				}),
			},
		});
		expect(screen.getByTestId('held-tool-label').textContent?.trim()).toBe('Tool unclear');
		expect(screen.queryByText('Sollomate Opalo')).toBeNull();
		expect(screen.queryByTestId('weapon-undeclared-cue')).toBeNull();
	});

	it('shows a tool the screen shows with the activity it implies', () => {
		render(OverlayReadout, {
			props: { data: liveData({ ...fallback, weaponDeclared: true, toolSource: pistol }) },
		});
		expect(screen.getByTestId('held-tool-label').textContent?.trim()).toBe('Pistol');
		expect(screen.getByTestId('activity-feedback').textContent?.trim()).toBe('Hunting');
	});

	it('keeps the weapon mismatch, with its decision, worded for the screen', async () => {
		const onWeaponDecision = vi.fn();
		render(OverlayReadout, {
			props: {
				data: liveData({
					...fallback,
					toolSource: pistol,
					weaponGuardrail: { hotbarTool: 'Pistol', recordingTool: 'Rifle', since: 1, shots: 3 },
				}),
				onWeaponDecision,
			},
		});
		const alert = screen.getByTestId('weapon-guardrail-alert');
		expect(alert.getAttribute('title')).toBe('Damage fits Rifle; the screen shows Pistol');
		expect(screen.queryByTestId('held-tool-facet')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm Rifle' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Keep Pistol' }));
		expect(onWeaponDecision.mock.calls).toEqual([['confirm'], ['keep']]);
	});

	it('keeps the harvest mismatch, worded for the screen', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({
					...fallback,
					currentToolKind: 'harvesting',
					toolSource: source({ kind: 'tool', name: 'Finder', provisional: false, setup: null }),
					harvestGuardrail: {
						expectedTool: 'Axe',
						observedTool: 'Finder',
						treeSize: 'huge',
						atEpoch: 1,
					},
				}),
			},
		});
		expect(screen.getByTestId('guardrail-alert').getAttribute('title')).toBe(
			'Board output says Axe; the screen shows Finder',
		);
	});

	it('keeps the hit cue under the tool, saying which set-up to check', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({
					...fallback,
					weaponDeclared: true,
					toolSource: pistol,
					weaponHitCue: 'reduced',
				}),
			},
		});
		const cue = screen.getByTestId('weapon-hit-cue');
		expect(cue.textContent?.trim()).toBe('Reduced');
		expect(cue.getAttribute('title')).toMatch(/check which set-up is on in Equipment\.$/);
		expect(screen.queryByTestId('activity-feedback')).toBeNull();
	});

	it('declares an entry the player picks where the screen cannot say', async () => {
		const onDeclareTool = vi.fn();
		render(OverlayReadout, {
			props: {
				data: liveData({
					...fallback,
					toolSource: source({
						kind: 'uncertain',
						why: { kind: 'disabled', entries: [{ id: 7, name: 'Rifle', label: 'Rifle' }] },
					}),
				}),
				onDeclareTool,
			},
		});
		await fireEvent.click(screen.getByTestId('held-tool'));
		expect(screen.getByText('Rifle is turned off in your equipment.')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Use Rifle' }));
		expect(onDeclareTool).toHaveBeenCalledWith(7);
	});

	it('opens the picker from "Choose a tool" through its own action', async () => {
		const onChooseTool = vi.fn();
		const onEquipmentTrigger = vi.fn();
		render(OverlayReadout, {
			props: {
				data: liveData({
					...fallback,
					toolSource: source({ kind: 'uncertain', why: { kind: 'unreadable' } }),
				}),
				onChooseTool,
				onEquipmentTrigger,
			},
		});
		await fireEvent.click(screen.getByTestId('held-tool'));
		await fireEvent.click(screen.getByRole('button', { name: 'Choose a tool' }));
		expect(onChooseTool).toHaveBeenCalledTimes(1);
		expect(onEquipmentTrigger).not.toHaveBeenCalled();
	});

	it('does not offer the picker while the screen shows what is held', () => {
		render(OverlayReadout, { props: { data: liveData({ ...fallback, toolSource: pistol }) } });
		const glyph = screen.getByTestId('equipment-icon') as HTMLButtonElement;
		expect(glyph.disabled).toBe(true);
		expect(glyph.getAttribute('title')).toBe('The screen shows what is held');
	});

	it('keeps the hotbar display and all its cues while the screen is not being read, with one marker', async () => {
		render(OverlayReadout, {
			props: {
				data: liveData({
					...fallback,
					toolSource: source(
						{ kind: 'notRead' },
						{ readingScreen: false, notReading: 'notStarted' },
					),
				}),
			},
		});
		expect(screen.getByText('Sollomate Opalo')).toBeTruthy();
		expect(screen.getByTestId('weapon-undeclared-cue').textContent?.trim()).toBe(
			'Press weapon slot',
		);
		expect(screen.queryByTestId('held-tool-facet')).toBeNull();
		expect(screen.getByTestId('screen-unread-marker').textContent?.trim()).toBe('Screen not read');
		await fireEvent.click(screen.getByTestId('screen-unread'));
		expect(
			screen.getByText(
				'Screen capture is not running: turn the setting off and on to start it. Hotbar keys are in use.',
			),
		).toBeTruthy();
		// Reading the held tool is not in development: the note carries no mark.
		expect(screen.queryByTestId('in-development-mark')).toBeNull();
	});

	it('keeps the healer display while the screen is not being read', () => {
		render(OverlayReadout, {
			props: {
				data: liveData({
					status: 'active',
					currentTool: 'FAP',
					currentToolKind: 'healing',
					toolSource: source({ kind: 'notRead' }, { readingScreen: false, notReading: 'stopped' }),
				}),
			},
		});
		expect(screen.getByText('FAP')).toBeTruthy();
		expect(screen.getByTestId('screen-unread')).toBeTruthy();
	});

	it('changes nothing while the keys are the source', () => {
		const keys = source({ kind: 'notRead' }, { readingScreen: false });
		const cases: Partial<TrackingLive>[] = [
			fallback,
			{ ...fallback, weaponDeclared: true, weaponHitCue: 'boosted' },
			{
				...fallback,
				weaponGuardrail: { hotbarTool: 'Pistol', recordingTool: 'Rifle', since: 1, shots: 3 },
			},
		];
		for (const data of cases) {
			const { container: withSource, unmount: a } = render(OverlayReadout, {
				props: { data: liveData({ ...data, toolSource: keys }) },
			});
			const marked = withSource.innerHTML;
			a();
			const { container: without, unmount: b } = render(OverlayReadout, {
				props: { data: liveData(data) },
			});
			expect(without.innerHTML).toBe(marked);
			b();
		}
	});
});

describe('the session title', () => {
	it('is the picker while idle: plain text that opens the sessions', () => {
		const onDefinitionTrigger = vi.fn();
		render(OverlayReadout, {
			props: { data: liveData({ sessionName: 'ARIS Dailies' }), onDefinitionTrigger },
		});
		const title = screen.getByRole('button', { name: 'ARIS Dailies' });
		title.click();
		expect(onDefinitionTrigger).toHaveBeenCalledWith(title);
	});

	it('is fixed text while a session runs', () => {
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', sessionName: 'ARIS Dailies' }) },
		});
		expect(screen.getByText('ARIS Dailies').tagName).toBe('SPAN');
		expect(screen.queryByRole('button', { name: 'ARIS Dailies' })).toBeNull();
	});

	it('says when the figures are the lifetime ones', () => {
		statsScope.current = 'lifetime';
		const lifetime = { instanceCount: 3 } as NonNullable<TrackingStatus['lifetime']>;
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active' }), status: activeStatus({ lifetime }) },
		});
		expect(screen.getByTestId('overlay-lifetime-marker').textContent?.trim()).toBe(
			'Showing lifetime',
		);
		statsScope.current = 'instance';
	});
});

describe('the mob while the screen is read', () => {
	const old = { name: 'Atrox Old', species: 'Atrox', maturity: 'Old' };
	const young = { name: 'Atrox Young', species: 'Atrox', maturity: 'Young' };

	function mount(source: NonNullable<TrackingLive['mobSource']>) {
		const deps = {
			view: () => source,
			pin: vi.fn(async () => ({})),
			unpin: vi.fn(async () => true),
			setTyped: vi.fn(async () => ({})),
			clearTyped: vi.fn(async () => ({})),
			refresh: vi.fn(async () => {}),
		};
		const mobSource = createMobSourceModel(deps);
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active', mobSource: source }), mobSource },
		});
		return deps;
	}

	it('shows the mob read now, species and maturity apart, and pins that one', async () => {
		const deps = mount({
			readingScreen: true,
			pin: null,
			typed: young,
			reading: old,
			continuity: null,
		});
		const shown = screen.getByTestId('mob-shown');
		expect(shown.dataset.standing).toBe('reading');
		expect(shown.querySelector('.species')?.textContent).toBe('Atrox');
		expect(shown.querySelector('.maturity')?.textContent).toBe('Old');
		await fireEvent.click(screen.getByRole('button', { name: 'Pin this mob' }));
		expect(deps.pin).toHaveBeenCalledWith({ kind: 'shown', creature: old });
	});

	it('releases a pinned mob', async () => {
		const deps = mount({
			readingScreen: false,
			pin: { creature: old, by: 'read' } as NonNullable<TrackingLive['mobSource']>['pin'],
			typed: null,
			reading: null,
			continuity: null,
		});
		expect(screen.getByTestId('mob-shown').dataset.standing).toBe('pinned');
		await fireEvent.click(screen.getByRole('button', { name: 'Release the pinned mob' }));
		expect(deps.unpin).toHaveBeenCalledTimes(1);
	});

	it('says where the mob comes from when expanded, with the backstop to set', async () => {
		mount({ readingScreen: true, pin: null, typed: young, reading: old, continuity: null });
		expect(screen.queryByText('Your backstop')).toBeNull();
		await fireEvent.click(screen.getByTestId('mob-shown'));
		expect(screen.getByText('Reading now')).toBeTruthy();
		expect(screen.getByText('Your backstop')).toBeTruthy();
		expect(screen.getByPlaceholderText('Type a mob...')).toBeTruthy();
	});
});

describe('the doses in force', () => {
	function dose(overrides: Record<string, unknown> = {}) {
		return {
			id: 'd1',
			equipmentId: 40,
			itemName: 'Adrenaline',
			source: 'hotbar',
			sessionId: 's1',
			startedAt: 1000,
			endsAt: 1600,
			untimed: false,
			replaced: false,
			costPed: 4.5,
			costTracked: true,
			effects: [],
			removedAt: null,
			removedBy: null,
			...overrides,
		};
	}

	function model(overrides: Record<string, unknown> = {}) {
		return {
			rows: [dose()],
			options: [{ equipmentId: 40, name: 'Adrenaline' }],
			undoable: null,
			now: 1000,
			busy: null,
			end: vi.fn(async () => true),
			remove: vi.fn(async () => true),
			start: vi.fn(async () => true),
			restore: vi.fn(async () => true),
			...overrides,
		};
	}

	it('shows each running dose with its countdown and the one correction', async () => {
		const doses = model();
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active' }), doses: doses as never },
		});
		const row = screen.getByTestId('overlay-dose');
		expect(row.textContent).toContain('Adrenaline');
		expect(row.textContent).toContain('10:00');
		await fireEvent.click(screen.getByLabelText('Remove this dose of Adrenaline'));
		expect(doses.remove).toHaveBeenCalledTimes(1);
	});

	it('ends an untimed dose instead of counting it down', async () => {
		const untimed = dose({ untimed: true, endsAt: null });
		const doses = model({ rows: [untimed] });
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active' }), doses: doses as never },
		});
		await fireEvent.click(screen.getByLabelText('End the effect of Adrenaline'));
		expect(doses.end).toHaveBeenCalledWith(untimed);
	});

	it('offers a removed dose back', async () => {
		const removed = dose();
		const doses = model({ rows: [], undoable: { dose: removed, at: 1000 } });
		render(OverlayReadout, {
			props: { data: liveData({ status: 'active' }), doses: doses as never },
		});
		await fireEvent.click(screen.getByText('Removed · Undo'));
		expect(doses.restore).toHaveBeenCalledWith(removed);
	});

	it('opens the consumables from the row and shows nothing while idle', async () => {
		const onDosesTrigger = vi.fn();
		const { unmount } = render(OverlayReadout, {
			props: { data: liveData({ status: 'active' }), doses: model() as never, onDosesTrigger },
		});
		await fireEvent.click(screen.getByLabelText('Take a dose'));
		expect(onDosesTrigger).toHaveBeenCalledTimes(1);
		unmount();
		render(OverlayReadout, { props: { data: liveData(), doses: model() as never } });
		expect(screen.queryByTestId('overlay-doses')).toBeNull();
	});
});

describe('the width', () => {
	const prefs = (ids: string[]) => ids.map((id) => ({ id, enabled: true }));

	it('follows the number of stats chosen, and two or fewer is the small form', () => {
		overlayStats.current = prefs(['net', 'rate']);
		const { container, unmount } = render(OverlayReadout, { props: { data: liveData() } });
		expect(container.querySelector('.readout')?.classList.contains('compact')).toBe(true);
		unmount();
		overlayStats.current = prefs(['cycled', 'loot_tt', 'net', 'rate', 'pes', 'dpp', 'kills_count']);
		const wide = render(OverlayReadout, { props: { data: liveData() } });
		const readout = wide.container.querySelector('.readout') as HTMLElement;
		expect(readout.classList.contains('compact')).toBe(false);
		// Seven stats widen the block themselves: none is cut at the edge.
		expect(readout.style.getPropertyValue('--content-max')).toBe(
			readout.style.getPropertyValue('--content-min'),
		);
	});
});
