/**
 * A short countdown that ends in one action: the calibration path for a
 * machine that cannot read keys. The player starts it, moves the mouse over
 * the point to capture, and the capture lands when it reaches zero, the way
 * an Enter press would have taken it.
 */

export function createCountdown(seconds: number, onElapsed: () => void | Promise<void>) {
	let remaining = $state<number | null>(null);
	let timer: ReturnType<typeof setTimeout> | null = null;

	function clear() {
		if (timer !== null) clearTimeout(timer);
		timer = null;
	}

	function tick() {
		if (remaining === null) return;
		if (remaining <= 1) {
			remaining = null;
			timer = null;
			void onElapsed();
			return;
		}
		remaining -= 1;
		timer = setTimeout(tick, 1000);
	}

	return {
		/** Seconds left, or null when not counting. */
		get remaining() {
			return remaining;
		},
		start() {
			clear();
			remaining = seconds;
			timer = setTimeout(tick, 1000);
		},
		cancel() {
			clear();
			remaining = null;
		},
	};
}
