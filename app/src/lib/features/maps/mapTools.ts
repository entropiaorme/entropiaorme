import type { MapPin } from '$lib/api';
import { rankByQuery } from '$lib/view/search';
import type { GamePoint } from './coords';

export interface MapFocusRequest {
	point: GamePoint;
	nonce: number;
}

/** Pins by name and notes, best match first. */
export function filterMapPins(pins: MapPin[], query: string): MapPin[] {
	return rankByQuery(pins, query, (pin) => [pin.name, pin.notes]);
}
