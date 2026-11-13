/**
 * The settings that read the game screen while a session runs: creature
 * detection and reading the held tool. Both are one control (a toggle, a
 * line of copy, and a note when capture cannot run), so they share one
 * component and are described here. Creature detection is still in
 * development: it carries its registered mark and the settings show it only
 * where in-development surfaces show. Reading the held tool is not: it is
 * unmarked and shown in every build, beside the hotbar key listener whose
 * part it takes over.
 */

import type { ScreenCapture, SettingsPatch } from '$lib/api';
import { inDevelopmentSurface } from '$lib/inDevelopment';
import type { AppSettings } from '$lib/types';

export interface ScreenReading {
	/** Stable key: names the control's saving and saved state. */
	readonly key: 'creatureDetection' | 'heldToolReading';
	/** The registered in-development surface the control is marked as;
	 * absent for a setting that is not in development. */
	readonly mark?: string;
	readonly title: string;
	readonly description: string;
	/** What the settings say when the change could not be saved. */
	readonly failure: string;
	/** Whether it is on, and whether it can capture, as the settings stand. */
	readonly state: (settings: AppSettings) => { enabled: boolean; capture: ScreenCapture };
	/** The patch that turns it on or off. */
	readonly patch: (on: boolean) => SettingsPatch;
}

export const CREATURE_DETECTION: ScreenReading = {
	key: 'creatureDetection',
	mark: inDevelopmentSurface('creature-detection').id,
	title: 'Detect the creature I am fighting',
	description:
		'While you hunt with the game in front, reads the name over the creature you are locked on to about once a second and records which creature each kill was. A kill it cannot tell takes the creature you have been hunting, else the mob you typed; pin a creature on the overlay to stop reading.',
	failure: 'Failed to update creature detection',
	state: (settings) => ({
		enabled: settings.mobDetectionEnabled,
		capture: settings.mobDetectionCapture,
	}),
	patch: (on) => ({ mob_detection_enabled: on }),
};

export const HELD_TOOL_READING: ScreenReading = {
	key: 'heldToolReading',
	title: 'Read the held tool from the screen',
	description:
		"The tool in hand is read from the game's own bar instead of the hotbar keys. Hotbar keys still start consumables.",
	failure: 'Failed to update reading the held tool',
	state: (settings) => ({
		enabled: settings.toolReadingEnabled,
		capture: settings.toolReadingCapture,
	}),
	patch: (on) => ({ tool_reading_enabled: on }),
};

export const SCREEN_READINGS: readonly ScreenReading[] = [CREATURE_DETECTION, HELD_TOOL_READING];

/**
 * What a screen-reading setting says beside its toggle when, turned on, it
 * still cannot watch: one plain line, or nothing when it can.
 */
export function captureNote(capture: ScreenCapture): string | null {
	switch (capture) {
		case 'ready':
			return null;
		case 'notRunning':
			return 'Screen capture is not running; turn this off and on to allow it.';
		case 'refused':
			return 'Screen capture was not allowed or has stopped; turn this off and on to ask again.';
		case 'unavailable':
			return 'This installation cannot read the game screen: the text recogniser or the interface description did not load.';
	}
}
