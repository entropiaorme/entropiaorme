import { describe, expect, it, vi } from 'vitest';

import { createPostSessionFlow, type PostSessionFlowOptions } from './postSession.svelte';

// The stop flow under test: the armour prompt gating the stop, Record
// leaving the session running, and Later stopping without opening any
// armour workflow. Every dependency is injected, so the transitions run for
// real against controllable seams.

function makeFlow(overrides: Partial<PostSessionFlowOptions> = {}) {
	// `options` keeps the concrete mock types; tests overriding a dependency
	// assert on their own mock, not through `options`.
	const options = {
		isSessionActive: vi.fn(() => true),
		isBusy: vi.fn(() => false),
		armourReminderEnabled: vi.fn(() => false),
		refresh: vi.fn(async () => {}),
		stopTracking: vi.fn(async () => ({ session_id: 's1' })),
		showArmourWorkflowInSession: vi.fn(async () => true),
	};
	const flow = createPostSessionFlow({ ...options, ...overrides } satisfies PostSessionFlowOptions);
	return { flow, options };
}

describe('requestStop', () => {
	it('does nothing when no session is active or a toggle is in flight', async () => {
		const inactive = makeFlow({ isSessionActive: vi.fn(() => false) });
		await inactive.flow.requestStop();
		expect(inactive.options.stopTracking).not.toHaveBeenCalled();
		expect(inactive.flow.awaitingArmourDecision).toBe(false);

		const busy = makeFlow({ isBusy: vi.fn(() => true) });
		await busy.flow.requestStop();
		expect(busy.options.stopTracking).not.toHaveBeenCalled();
	});

	it('arms the armour prompt instead of stopping when the reminder is enabled', async () => {
		const { flow, options } = makeFlow({ armourReminderEnabled: vi.fn(() => true) });

		await flow.requestStop();
		expect(flow.awaitingArmourDecision).toBe(true);
		expect(options.stopTracking).not.toHaveBeenCalled();
		expect(flow.stopping).toBe(false);
	});

	it('stops straight away when the reminder is disabled', async () => {
		const { flow, options } = makeFlow();

		await flow.requestStop();
		expect(options.stopTracking).toHaveBeenCalledTimes(1);
		expect(options.showArmourWorkflowInSession).not.toHaveBeenCalled();
	});
});

describe('the stop sequence', () => {
	it('stops, then re-reads the snapshot', async () => {
		const { flow, options } = makeFlow();

		await flow.requestStop();

		expect(options.refresh).toHaveBeenCalledTimes(1);
		expect(options.stopTracking.mock.invocationCallOrder[0]).toBeLessThan(
			options.refresh.mock.invocationCallOrder[0],
		);
	});

	it('swallows a stop failure and still ends the flow', async () => {
		const { flow } = makeFlow({
			stopTracking: vi.fn(async () => {
				throw new Error('backend away');
			}),
		});

		await flow.requestStop();
		expect(flow.stopping).toBe(false);
	});

	it('flags stopping for the duration of the stop', async () => {
		let resolveStop!: (value: { session_id: string }) => void;
		const { flow } = makeFlow({
			stopTracking: vi.fn(
				() =>
					new Promise<{ session_id: string }>((resolve) => {
						resolveStop = resolve;
					}),
			),
		});

		const pending = flow.requestStop();
		await Promise.resolve();
		expect(flow.stopping).toBe(true);
		resolveStop({ session_id: 's1' });
		await pending;
		expect(flow.stopping).toBe(false);
	});
});

describe('decideArmourTrack', () => {
	it('does nothing when the prompt is not armed', async () => {
		const { flow, options } = makeFlow();
		await flow.decideArmourTrack('yes');
		expect(options.stopTracking).not.toHaveBeenCalled();
		expect(options.showArmourWorkflowInSession).not.toHaveBeenCalled();
	});

	it('Record opens the workflow against the session and leaves it running', async () => {
		// Armour cost belongs to the session it was spent in, so recording it
		// is part of that session rather than an afterthought about a closed
		// one. The user stops when they have finished.
		const { flow, options } = makeFlow({ armourReminderEnabled: vi.fn(() => true) });
		await flow.requestStop();

		await flow.decideArmourTrack('yes');
		expect(flow.awaitingArmourDecision).toBe(false);
		expect(options.showArmourWorkflowInSession).toHaveBeenCalledTimes(1);
		expect(options.stopTracking).not.toHaveBeenCalled();
	});

	it('a second stop after recording still offers the prompt', async () => {
		const { flow, options } = makeFlow({ armourReminderEnabled: vi.fn(() => true) });
		await flow.requestStop();
		await flow.decideArmourTrack('yes');

		await flow.requestStop();
		expect(flow.awaitingArmourDecision).toBe(true);
		await flow.decideArmourTrack('no');
		expect(options.stopTracking).toHaveBeenCalledTimes(1);
	});

	it('Later stops and opens no armour workflow', async () => {
		// The user has just said the armour can wait: nothing opens over the
		// stop, whatever the session's protection attribution.
		const { flow, options } = makeFlow({ armourReminderEnabled: vi.fn(() => true) });
		await flow.requestStop();

		await flow.decideArmourTrack('no');
		expect(flow.awaitingArmourDecision).toBe(false);
		expect(options.stopTracking).toHaveBeenCalledTimes(1);
		expect(options.showArmourWorkflowInSession).not.toHaveBeenCalled();
	});
});
