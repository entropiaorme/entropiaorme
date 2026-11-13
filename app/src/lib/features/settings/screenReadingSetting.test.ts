import { describe, expect, it } from 'vitest';
import { IN_DEVELOPMENT_SURFACES } from '$lib/inDevelopment';
import type { AppSettings } from '$lib/types';
import { captureNote, SCREEN_READINGS } from './screenReadingSetting';

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

	it('marks each control as its own registered in-development surface', () => {
		const registered = IN_DEVELOPMENT_SURFACES.map((surface) => surface.id);
		expect(SCREEN_READINGS.map((reading) => reading.mark)).toEqual([
			'creature-detection',
			'held-tool-reading',
		]);
		for (const reading of SCREEN_READINGS) expect(registered).toContain(reading.mark);
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

	it("names the held-tool reading in the player's terms", () => {
		expect(heldTool.title).toBe('Read the held tool from the screen');
		expect(heldTool.description).toBe(
			"The tool in hand is read from the game's own bar instead of the hotbar keys. Hotbar keys still start consumables.",
		);
	});
});
