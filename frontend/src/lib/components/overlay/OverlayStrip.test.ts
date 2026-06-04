// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import type { StatId } from '$lib/statsRegistry';
import { getStatDef } from '$lib/statsRegistry';

// The strip renders from props plus the overlayStats customisation store; the
// store is the one side-effecting seam (its real module pulls in the Tauri
// preference plumbing), so it is replaced with a hand-rolled store-contract
// stub the tests drive (vi.hoisted: the factory runs before top-level imports
// initialise, so svelte/store's writable is not constructible there). The
// stats registry is real: the pill assertions exercise the actual render
// functions.
const { overlayStats } = vi.hoisted(() => {
	type Pref = { id: string; enabled: boolean };
	let value: Pref[] = [];
	const subscribers = new Set<(v: Pref[]) => void>();
	return {
		overlayStats: {
			set(next: Pref[]): void {
				value = next;
				for (const fn of subscribers) fn(value);
			},
			subscribe(fn: (v: Pref[]) => void): () => void {
				subscribers.add(fn);
				fn(value);
				return () => subscribers.delete(fn);
			},
		},
	};
});

vi.mock('$lib/statsCustomisation', () => ({
	overlayStats,
}));

import OverlayStrip from './OverlayStrip.svelte';

import type { TrackingLive, TrackingStatus } from '$lib/api';

function liveData(overrides: Partial<TrackingLive> = {}): TrackingLive {
	return { status: 'idle', ...overrides };
}

function activeStatus(overrides: Partial<TrackingStatus> = {}): TrackingStatus {
	return { status: 'active', ...overrides };
}

beforeEach(() => {
	overlayStats.set([]);
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

		expect(screen.getByText('Track armour?')).toBeTruthy();
		expect(screen.queryByTitle('Stop tracking')).toBeNull();

		screen.getByText('Yes').click();
		expect(onArmourTrackDecision).toHaveBeenCalledWith('yes');
		screen.getByText('No').click();
		expect(onArmourTrackDecision).toHaveBeenCalledWith('no');
	});

	it('does not interpose when the session is not active', () => {
		render(OverlayStrip, {
			props: { data: liveData(), awaitingArmourTrackDecision: true },
		});
		expect(screen.queryByText('Track armour?')).toBeNull();
		expect(screen.getByTitle('Start tracking')).toBeTruthy();
	});
});

describe('attribution warning', () => {
	it('replaces TRACK while idle and dismisses through the callback', () => {
		const onDismissAttributionWarning = vi.fn();
		render(OverlayStrip, {
			props: {
				data: liveData(),
				attributionWarning: 'Configure a weapon before tracking',
				onDismissAttributionWarning,
			},
		});

		expect(screen.getByText('Configure a weapon before tracking')).toBeTruthy();
		expect(screen.queryByTitle('Start tracking')).toBeNull();

		screen.getByLabelText('Dismiss warning').click();
		expect(onDismissAttributionWarning).toHaveBeenCalledTimes(1);
	});

	it('does not replace the stop control during an active session', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active' }),
				attributionWarning: 'Configure a weapon before tracking',
			},
		});
		expect(screen.queryByText('Configure a weapon before tracking')).toBeNull();
		expect(screen.getByTitle('Stop tracking')).toBeTruthy();
	});
});

describe('mob and tag section', () => {
	it('locks the mode toggle during an active session', () => {
		render(OverlayStrip, { props: { data: liveData({ status: 'active' }) } });
		expect((screen.getByText('MOB') as HTMLButtonElement).disabled).toBe(true);
		expect((screen.getByText('TAG') as HTMLButtonElement).disabled).toBe(true);
	});

	it('forwards a mode change while idle', () => {
		const onMobModeChange = vi.fn();
		render(OverlayStrip, { props: { data: liveData(), onMobModeChange } });

		screen.getByText('TAG').click();
		expect(onMobModeChange).toHaveBeenCalledWith('tag');
		screen.getByText('MOB').click();
		expect(onMobModeChange).toHaveBeenCalledWith('mob');
	});

	it('shows the tag input when tag mode has no locked tag', () => {
		render(OverlayStrip, {
			props: { data: liveData({ mobEntryMode: 'tag', currentMob: null }) },
		});
		expect(screen.getByPlaceholderText('Tag...')).toBeTruthy();
	});

	it('shows the mob input when mob mode has no locked mob', () => {
		render(OverlayStrip, {
			props: { data: liveData({ mobEntryMode: 'mob', currentMob: null }) },
		});
		expect(screen.getByPlaceholderText('Mob...')).toBeTruthy();
	});

	it('hides the release control when no mob is locked', () => {
		render(OverlayStrip, {
			props: { data: liveData({ mobEntryMode: 'mob', currentMob: null }) },
		});
		expect(screen.queryByLabelText('Release mob')).toBeNull();
	});

	it('shows the locked mob with a release control instead of the input', () => {
		const onReleaseMob = vi.fn();
		render(OverlayStrip, {
			props: { data: liveData({ mobEntryMode: 'mob', currentMob: 'Atrox Young' }), onReleaseMob },
		});

		expect(screen.getByText('Atrox Young')).toBeTruthy();
		expect(screen.queryByPlaceholderText('Mob...')).toBeNull();

		screen.getByLabelText('Release mob').click();
		expect(onReleaseMob).toHaveBeenCalledTimes(1);
	});

	it('surfaces the popup launch error under the input when the menu is closed', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ mobEntryMode: 'tag' }),
				overlayMenuLaunchError: 'Popup route did not become ready',
				mobMenuOpen: false,
			},
		});
		expect(screen.getByText('Popup route did not become ready')).toBeTruthy();
	});
});

describe('customisable stat pills', () => {
	it('renders only the enabled overlay stats, through the real registry render', () => {
		overlayStats.set([
			{ id: 'net' as StatId, enabled: true },
			{ id: 'kills' as StatId, enabled: false },
		]);
		const status = activeStatus({ cost: 10, returns: 12.5, kill_count: 7 });
		render(OverlayStrip, { props: { data: liveData({ status: 'active' }), status } });

		const netDef = getStatDef('net' as StatId);
		const killsDef = getStatDef('kills' as StatId);
		expect(netDef && screen.getByText(netDef.label)).toBeTruthy();
		expect(killsDef && screen.queryByText(killsDef.label)).toBeNull();
		// net = returns - cost, rendered by the registry's own formatter.
		expect(screen.getByText(netDef!.render(status).value)).toBeTruthy();
	});

	it('renders nothing when no overlay stat is enabled', () => {
		overlayStats.set([{ id: 'net' as StatId, enabled: false }]);
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

	it('surfaces the trifecta error under the trigger', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ weaponAttribution: 'trifecta', trifectaAttribution: trifecta }),
				trifectaError: 'Popup route did not become ready',
			},
		});
		expect(screen.getByText('Popup route did not become ready')).toBeTruthy();
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

	it('surfaces the armour cost error while the popup is closed', () => {
		render(OverlayStrip, {
			props: {
				data: liveData({ status: 'active' }),
				armourSessionId: 's1',
				armourCostError: 'Armour cost popup did not become ready',
				armourCostOpen: false,
			},
		});
		expect(screen.getByText('Armour cost popup did not become ready')).toBeTruthy();
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

	it('offers the quest-link suggestion and forwards the decision', () => {
		const onQuestLinkDecision = vi.fn();
		render(OverlayStrip, {
			props: {
				...postSession,
				questLinkSuggestion: {
					sessionId: 's1',
					suggestionType: 'quest',
					reason: 'single_quest',
					questId: 'q1',
					questName: 'Iron Challenge',
					playlistId: null,
					playlistName: null,
				},
				onQuestLinkDecision,
			},
		});

		expect(screen.getByText('Iron Challenge')).toBeTruthy();
		screen.getByText('Yes').click();
		expect(onQuestLinkDecision).toHaveBeenCalledWith('accept');
		screen.getByText('No').click();
		expect(onQuestLinkDecision).toHaveBeenCalledWith('decline');
	});

	it('shows the quest-link outcome message with a dismiss control', () => {
		const onDismissQuestLinkMessage = vi.fn();
		render(OverlayStrip, {
			props: {
				...postSession,
				questLinkMessage: 'Linked to Iron Challenge',
				onDismissQuestLinkMessage,
			},
		});

		expect(screen.getByText('Linked to Iron Challenge')).toBeTruthy();
		screen.getByText('Done').click();
		expect(onDismissQuestLinkMessage).toHaveBeenCalledTimes(1);
	});
});
