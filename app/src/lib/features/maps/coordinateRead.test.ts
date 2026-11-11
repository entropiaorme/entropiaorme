import { describe, expect, it } from 'vitest';
import { coordRefusalMessage, scannedPoint } from './coordinateRead';

describe('a coordinate read', () => {
	it('gives a position only when both values read', () => {
		expect(scannedPoint({ kind: 'read', lon: 61234, lat: 75456, source: 'radar' })).toEqual({
			lon: 61234,
			lat: 75456,
		});
		expect(
			scannedPoint({ kind: 'refused', refusal: { reason: 'notRead', lon: null, lat: 'unclear' } }),
		).toBeNull();
	});

	it('names the refusal the player can act on first', () => {
		expect(
			coordRefusalMessage({ reason: 'notRead', lon: 'unclear', lat: 'covered' }, 'Calypso'),
		).toBe('The coordinates are covered by another window.');
		expect(
			coordRefusalMessage({ reason: 'notRead', lon: 'outOfBounds', lat: null }, 'Calypso'),
		).toBe('The coordinates shown are outside Calypso.');
		expect(coordRefusalMessage({ reason: 'planetUnknown', lon: null, lat: null }, 'Thule')).toBe(
			'Thule has no known coordinates to read against.',
		);
		expect(
			coordRefusalMessage({ reason: 'captureNotAllowed', lon: null, lat: null }, 'Calypso'),
		).toBe('Screen capture was not allowed.');
		expect(coordRefusalMessage({ reason: 'otherPlanet', lon: null, lat: null }, 'Calypso')).toBe(
			'The radar names another planet than Calypso.',
		);
	});
});
