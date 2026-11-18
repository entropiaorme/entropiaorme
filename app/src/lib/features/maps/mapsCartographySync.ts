import { listen } from '@tauri-apps/api/event';
import { MAP_PINS_CHANGED_EVENT } from './cartographyOverlay.svelte';
import type { MapsModel } from './mapsModel.svelte';

/**
 * Refreshes the Maps view model's pins when the cartography overlay window
 * drops one, for one mounted maps-surface lifetime. Selection is owned by the
 * main surface and published to the overlay from it (see the context
 * broadcast in `MapsSurface.svelte`); this only handles the overlay-to-map pin
 * feedback.
 */
export function startMapsCartographySync(model: MapsModel): () => void {
	let mounted = true;
	let unlistenPins: (() => void) | undefined;

	void (async () => {
		const stopPins = await listen<{ planet?: string }>(MAP_PINS_CHANGED_EVENT, (event) => {
			if (event.payload?.planet === model.selected?.name) void model.refreshPins();
		});
		if (!mounted) {
			stopPins();
			return;
		}
		unlistenPins = stopPins;
	})();

	return () => {
		mounted = false;
		unlistenPins?.();
	};
}
