import { describe, expect, it } from 'vitest';
import { IN_DEVELOPMENT_SURFACES } from '$lib/inDevelopment';
import type { AppSettings } from '$lib/types';
import {
	CREATURE_DETECTION,
	captureNote,
	HELD_TOOL_READING,
	SCREEN_READINGS,
} from './screenReadingSetting';

describe('the screen-reading capture note', () => {
	it('says nothing while capture can run, and one plain line when it cannot', () => {
		expect(captureNote('ready')).toBeNull();
		expect(captureNote('notRunning')).toBe(
			'Screen capture is not running; turn this off and on to allow it.',
		);
		expect(captureNote('refused')).toMatch(/turn this off and on to ask again/);
		expect(captureNote('unavailable')).toMatch(/cannot read the game screen/);
	});
});

describe('the screen-reading settings', () => {
	const settings = {
		mobDetectionEnabled: false,
		mobDetectionCapture: 'refused',
		toolReadingEnabled: true,
		toolReadingCapture: 'notRunning',
	} as AppSettings;

	const [creatures, heldTool] = SCREEN_READINGS;

	it('describes both settings, each under its own key', () => {
		expect(SCREEN_READINGS).toEqual([CREATURE_DETECTION, HELD_TOOL_READING]);
		expect(creatures).toBe(CREATURE_DETECTION);
		expect(heldTool).toBe(HELD_TOOL_READING);
		expect(new Set(SCREEN_READINGS.map((reading) => reading.key)).size).toBe(2);
	});

	it('leaves creature detection unmarked: it is not in development', () => {
		expect(creatures.mark).toBeUndefined();
		expect(IN_DEVELOPMENT_SURFACES.map((surface) => surface.id)).not.toContain(
			'creature-detection',
		);
	});

	it('leaves reading the held tool unmarked: it is not in development', () => {
		expect(heldTool.mark).toBeUndefined();
		expect(IN_DEVELOPMENT_SURFACES.map((surface) => surface.id)).not.toContain('held-tool-reading');
	});

	it('reads each setting from its own fields', () => {
		expect(creatures.state(settings)).toEqual({ enabled: false, capture: 'refused' });
		expect(heldTool.state(settings)).toEqual({ enabled: true, capture: 'notRunning' });
	});

	it('turns each on and off through its own patch field', () => {
		expect(creatures.patch(true)).toEqual({ mob_detection_enabled: true });
		expect(heldTool.patch(true)).toEqual({ tool_reading_enabled: true });
		expect(heldTool.patch(false)).toEqual({ tool_reading_enabled: false });
	});

	it("names creature detection in the player's terms, as it always has", () => {
		expect(creatures.title).toBe('Detect the creature I am fighting');
		expect(creatures.description).toBe(
			'While you hunt with the game in front, reads the name over the creature you are locked on to about once a second and records which creature each kill was. A kill it misses between looks takes the creature you have been hunting while you hunt only one, else the mob you typed; pin a creature on the overlay to stop reading.',
		);
		expect(creatures.failure).toBe('Failed to update creature detection');
		expect(creatures.patch(false)).toEqual({ mob_detection_enabled: false });
	});

	it("names the held-tool reading in the player's terms", () => {
		expect(heldTool.title).toBe('Read the held tool from the screen');
		expect(heldTool.description).toBe(
			"The tool in hand is read from the game's own bar instead of the hotbar keys. Hotbar keys still start consumables.",
		);
	});
});
