/**
 * What the creature detection setting says beside its toggle when, turned
 * on, it still cannot watch: one plain line, or nothing when it can.
 */

import type { MobDetectionCapture } from '$lib/api';

export function captureNote(capture: MobDetectionCapture): string | null {
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
