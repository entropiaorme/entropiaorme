<script lang="ts">
	/**
	 * Test fixture: a minimal consumer of the real trackingStore, mirroring the
	 * canonical subscribe-then-hydrate mount discipline (listener attached
	 * before the first read, so a frame landing mid-setup is never lost). The
	 * component suite drives the mocked API and Tauri listener seams and
	 * asserts this rendered output tracks the store.
	 */
	import { hydrate, subscribeTracking, trackingSnapshot } from '$lib/stores/trackingStore';

	$effect(() => {
		let unlisten: (() => void) | undefined;
		let disposed = false;
		void subscribeTracking().then((fn) => {
			if (disposed) {
				fn();
				return;
			}
			unlisten = fn;
			void hydrate();
		});
		return () => {
			disposed = true;
			unlisten?.();
		};
	});
</script>

<span data-testid="status">
	{$trackingSnapshot ? `${$trackingSnapshot.status}:${$trackingSnapshot.kill_count ?? 0}` : 'unhydrated'}
</span>
