import { describe, expect, it } from 'vitest';
import { captureNote } from './mobDetectionSetting';

describe('the creature detection capture note', () => {
	it('says nothing while capture can run, and one plain line when it cannot', () => {
		expect(captureNote('ready')).toBeNull();
		expect(captureNote('notRunning')).toBe(
			'Screen capture is not running; turn this off and on to allow it.',
		);
		expect(captureNote('refused')).toMatch(/turn this off and on to ask again/);
		expect(captureNote('unavailable')).toMatch(/cannot read the game screen/);
	});
});
