import { describe, expect, it, vi } from 'vitest';

import { createPostSessionFlow, type PostSessionFlowOptions } from './postSession.svelte';

// The stop flow under test: a stop that asks nothing, re-reads the snapshot
// once it lands, and survives a refused stop. Every dependency is injected,
// so the transitions run for real against controllable seams.

function makeFlow(overrides: Partial<PostSessionFlowOptions> = {}) {
	// `options` keeps the concrete mock types; tests overriding a dependency
	// assert on their own mock, not through `options`.
	const options = {
		isSessionActive: vi.fn(() => true),
		isBusy: vi.fn(() => false),
		refresh: vi.fn(async () => {}),
		stopTracking: vi.fn(async () => ({ session_id: 's1' })),
	};
	const flow = createPostSessionFlow({ ...options, ...overrides } satisfies PostSessionFlowOptions);
	return { flow, options };
}

describe('requestStop', () => {
	it('does nothing when no session is active or a toggle is in flight', async () => {
		const inactive = makeFlow({ isSessionActive: vi.fn(() => false) });
		await inactive.flow.requestStop();
		expect(inactive.options.stopTracking).not.toHaveBeenCalled();

		const busy = makeFlow({ isBusy: vi.fn(() => true) });
		await busy.flow.requestStop();
		expect(busy.options.stopTracking).not.toHaveBeenCalled();
	});

	it('stops straight away, asking nothing', async () => {
		const { flow, options } = makeFlow();

		await flow.requestStop();
		expect(options.stopTracking).toHaveBeenCalledTimes(1);
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
