/**
 * What the player is told when their coordinates could not be read off
 * the screen, and what a read gives them when it could. A refusal never
 * carries a number, so a refused read can only ever produce a message.
 */

import type { CoordRefusal, CoordScanResult, CoordValueRefusal } from '$lib/api';
import type { GamePoint } from './coords';

/** The position a scan read, or null when it was refused. */
export function scannedPoint(result: CoordScanResult): GamePoint | null {
	return result.kind === 'read' ? { lon: result.lon, lat: result.lat } : null;
}

// When both values are refused for different reasons, the one the player
// can most readily act on is named.
const VALUE_ORDER: CoordValueRefusal[] = [
	'covered',
	'offScreen',
	'notFound',
	'outOfBounds',
	'unclear',
];

function valueMessage(refusal: CoordValueRefusal, planet: string): string {
	switch (refusal) {
		case 'covered':
			return 'The coordinates are covered by another window.';
		case 'offScreen':
			return 'The coordinates under the radar are off the screen.';
		case 'notFound':
			return 'No coordinates are shown under the radar.';
		case 'outOfBounds':
			return `The coordinates shown are outside ${planet}.`;
		case 'unclear':
			return 'The coordinates did not read cleanly.';
	}
}

/** One plain sentence saying why no position was read. */
export function coordRefusalMessage(refusal: CoordRefusal, planet: string): string {
	switch (refusal.reason) {
		case 'readerUnavailable':
			return 'Coordinates cannot be read on this installation.';
		case 'planetUnknown':
			return `${planet} has no known coordinates to read against.`;
		case 'gameWindowNotFound':
			return 'The game window was not found.';
		case 'gameWindowOffScreen':
			return 'The game window is off the screen that can be captured.';
		case 'captureFailed':
			return 'The screen could not be captured.';
		case 'captureNotAllowed':
			return 'Screen capture was not allowed.';
		case 'radarNotFound':
			return 'The radar is not on the screen.';
		case 'otherPlanet':
			return `The radar names another planet than ${planet}.`;
		case 'notRead': {
			const refused = [refusal.lon, refusal.lat].filter(
				(value): value is CoordValueRefusal => value != null,
			);
			const first = VALUE_ORDER.find((value) => refused.includes(value)) ?? 'unclear';
			return valueMessage(first, planet);
		}
	}
}
