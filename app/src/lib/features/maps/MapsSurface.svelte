<script lang="ts">
	/**
	 * The whole Maps surface, hosted as the dashboard's map widget: the
	 * pan/zoom viewer, a compact action bar carrying the feedback line, the
	 * pin lifecycle and the setup dialogs, wired to the feature model and
	 * controllers.
	 */
	import { onMount } from 'svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import CalibrationModal from './CalibrationModal.svelte';
	import CartographyOverlayModal from './CartographyOverlayModal.svelte';
	import MapControls from './MapControls.svelte';
	import { startMapsCartographySync } from './mapsCartographySync';
	import MapViewer from './MapViewer.svelte';
	import RadarCalibrationModal from './RadarCalibrationModal.svelte';
	import PinEditModal from './PinEditModal.svelte';
	import { createMapsModel } from './mapsModel.svelte';
	import { createMapsController } from './mapsController.svelte';
	import { createMapAreaSelectionController } from './mapPinSelectionController.svelte';
	import type { GamePoint } from './coords';
	import type { MapFocusRequest } from './mapTools';
	import type { MapView, NavigationRun } from '$lib/api';
	import { describeError } from '$lib/view/errorState';
	import {
		getNavigationSnapshot,
		showNavigationOverlays,
		toggleCartographyOverlay,
	} from '$lib/api';
	import { listen } from '@tauri-apps/api/event';
	import {
		broadcastCartographyContext,
		CARTOGRAPHY_OVERLAY_CONTEXT_REQUEST,
	} from './cartographyOverlay.svelte';
	import { getPreference, setPreference } from '$lib/preferences';

	let { class: className = '' }: { class?: string } = $props();

	const LAST_PLANET_KEY = 'mapsLastPlanet';
	const LAST_MAP_VIEW_KEY = 'mapsLastMapViewId';

	const model = createMapsModel();
	const controller = createMapsController(model);
	const areaSelection = createMapAreaSelectionController(model, controller.flash);
	let navigation = $state<NavigationRun | null>(null);

	const selectedMapName = $derived(
		model.selectedViewId === null
			? 'Default'
			: model.views.find((view) => view.id === model.selectedViewId)?.name ?? 'Selected map',
	);

	onMount(() => {
		const stopMapsSync = startMapsCartographySync(model);
		const stopRouteAreaSelection = areaSelection.mount();
		// A listen() promise can resolve after the component has already
		// unmounted (rapid navigation away); stop the listener immediately in
		// that case rather than storing a handle the cleanup has already passed.
		let mounted = true;
		let unlisten: (() => void) | undefined;
		let unlistenContextRequest: (() => void) | undefined;
		// The overlay asks for the current context when it comes alive or is
		// shown (its one-shot broadcast may have fired before its listener was
		// live); reply with the live selection so its palette tracks the map.
		void listen(CARTOGRAPHY_OVERLAY_CONTEXT_REQUEST, () => publishContext()).then((stop) => {
			if (mounted) unlistenContextRequest = stop;
			else stop();
		});
		// Reopen the planet and named map the user last visited.
		void Promise.all([
			getPreference<string | null>(LAST_PLANET_KEY, null),
			getPreference<number | null>(LAST_MAP_VIEW_KEY, null),
		]).then(([planet, viewId]) => model.loadPlanets({ planet, viewId }));
		void getNavigationSnapshot().then((run) => {
			navigation = run;
			if (run?.status === 'active') void showNavigationOverlays();
		}).catch(() => {});
		void listen('navigation:updated', () => {
			void getNavigationSnapshot().then((run) => {
				const wasActive = navigation?.status === 'active';
				navigation = run;
				// A route just started (from the overlay's setup panel): position the
				// HUD and radar around the live run.
				const nowActive = run?.status === 'active';
				if (nowActive && !wasActive) void showNavigationOverlays();
			}).catch(() => {});
			// A recorded visit changes a pin's cooldown, so refresh the pins the
			// hover cards read from.
			void model.refreshPins();
		}).then((stop) => {
			if (mounted) unlisten = stop;
			else stop();
		});
		return () => {
			mounted = false;
			stopMapsSync();
			stopRouteAreaSelection();
			unlisten?.();
			unlistenContextRequest?.();
		};
	});

	// Publish the current planet/map-view context to the overlay window. The
	// single home for building the context payload, so the selection effect, the
	// overlay's on-show request reply, and the explicit re-publishes below all
	// broadcast the same shape.
	function publishContext() {
		broadcastCartographyContext({
			planet: model.selected?.name ?? null,
			mapViewId: model.selectedViewId,
		});
	}

	// Publish the active context whenever the selection changes, so the overlay's
	// palette tracks the map on screen.
	$effect(() => {
		publishContext();
	});

	// Remember the last-visited planet and named map so the surface reopens to
	// it next session. Reacts to every selection change (planet, view, add or
	// remove), so persistence lives in one place.
	$effect(() => {
		const planet = model.selected?.name ?? null;
		const viewId = model.selectedViewId;
		if (!planet) return;
		void setPreference(LAST_PLANET_KEY, planet);
		void setPreference(LAST_MAP_VIEW_KEY, viewId);
	});

	// A configuration change can restyle or remove placed pins; refresh them and
	// re-publish the context so the overlay reloads its palette.
	function onConfigsChanged() {
		void model.refreshPins();
		publishContext();
	}

	// Re-publish the context when the overlay is toggled visible: a pre-spawned
	// overlay shown after the last selection change would otherwise still hold
	// the context from whenever its listener last caught a broadcast.
	async function toggleOverlay() {
		await toggleCartographyOverlay();
		publishContext();
	}

	let calibrationOpen = $state(false);
	let overlayConfigOpen = $state(false);
	let radarCalibrationOpen = $state(false);

	// Route planning happens in the pre-spawned HUD overlay (not a modal on this
	// surface) so a single-monitor player can plan while the game is fullscreen.
	// Publish the current planet/map context, then show the overlay in setup mode.
	async function openRouteSetup() {
		publishContext();
		await showNavigationOverlays();
	}
	let focusRequest = $state<MapFocusRequest | null>(null);
	let focusNonce = 0;

	async function selectPlanet(name: string) {
		focusRequest = null;
		await model.selectPlanet(name);
		areaSelection.reconcileContext();
	}

	async function selectView(id: number | null) {
		await model.selectView(id);
		areaSelection.reconcileContext();
	}

	async function addView(): Promise<MapView | null> {
		try {
			return await model.addView();
		} catch (e) {
			flash(describeError(e, 'The map could not be created'));
			return null;
		}
	}

	async function renameView(id: number, name: string): Promise<boolean> {
		try {
			await model.renameView(id, name);
			// The view id is unchanged, so the selection effect does not fire;
			// re-publish the context so the overlay reloads the renamed view.
			publishContext();
			return true;
		} catch (e) {
			flash(describeError(e, 'The map could not be renamed'));
			return false;
		}
	}

	async function deleteView(view: MapView): Promise<boolean> {
		try {
			await model.removeView(view.id);
			return true;
		} catch (e) {
			flash(describeError(e, 'The map could not be deleted'));
			return false;
		}
	}

	function focusMap(point: GamePoint) {
		focusRequest = { point, nonce: ++focusNonce };
	}

	// The pin lifecycle (form state, create/edit/delete/copy, feedback) lives in
	// the controller; the surface flashes view-selection errors through it too.
	const flash = controller.flash;
</script>

<div class="flex min-h-0 flex-col gap-2 {className}" data-testid="maps-surface">
	<div class="flex min-h-8 shrink-0 items-center justify-between gap-3">
		<!-- The live region stays mounted so each flash is announced, not just
			 the first one that happens to create it. -->
		<p class="min-w-0 flex-1 truncate text-xs text-text-secondary" role="status">{controller.feedback ?? ''}</p>
		{#if model.planets.length > 0}
			<MapControls
				pins={model.pins}
				disabled={areaSelection.active}
				ontoggleoverlay={() => void toggleOverlay()}
				onconfigure={() => (overlayConfigOpen = true)}
				oncalibrate={() => (calibrationOpen = true)}
				onselectpin={(pin) => focusMap({ lon: pin.lon, lat: pin.lat })}
				onroute={() => void openRouteSetup()}
				onselectpins={areaSelection.beginPinSelection}
				onradarcalibrate={() => (radarCalibrationOpen = true)}
			/>
		{/if}
	</div>

	{#if model.error}
		<ErrorNotice message={model.error} />
	{:else if !model.loading && model.planets.length === 0}
		<p class="text-sm text-text-secondary">
			No planet maps are bundled with this installation, so the maps surface is unavailable.
		</p>
	{/if}

	<div class="min-h-0 flex-1">
		{#if model.selected && model.imageUrl}
			<MapViewer
				planet={model.selected}
				planets={model.planets}
				imageUrl={model.imageUrl}
				pins={model.pins}
				views={model.views}
				selectedViewId={model.selectedViewId}
				{focusRequest}
				{navigation}
				selectionMode={areaSelection.mode}
				selectionRegions={areaSelection.regions}
				onselectionregionschange={areaSelection.setRegions}
				onselectionclear={areaSelection.clearRegions}
				onselectioncancel={areaSelection.cancel}
				onselectionconfirm={areaSelection.confirmRoute}
				onselectiondelete={areaSelection.deletePins}
				onselectioncooldown={areaSelection.cooldownPins}
				onmapclick={controller.openDropForm}
				oncopywaypoint={controller.copyWaypoint}
				oneditpin={controller.openEditForm}
				ondeletepin={controller.deletePin}
				oncooldownpin={controller.cooldownPin}
				onselectplanet={(name) => void selectPlanet(name)}
				onselectview={(id) => void selectView(id)}
				onaddview={addView}
				onrenameview={renameView}
				ondeleteview={deleteView}
			/>
		{:else if model.loading}
			<div
				class="flex h-full items-center justify-center rounded-lg border border-border bg-base text-sm text-text-secondary"
			>
				Loading map…
			</div>
		{/if}
	</div>
</div>

<CartographyOverlayModal
	bind:open={overlayConfigOpen}
	planet={model.selected?.name ?? null}
	mapViewId={model.selectedViewId}
	mapName={selectedMapName}
	onchanged={onConfigsChanged}
/>

<PinEditModal
	bind:open={controller.formOpen}
	point={controller.dropPoint}
	editing={controller.editingPin}
	planet={model.selected?.name ?? null}
	mapViewId={model.selectedViewId}
	onsubmit={controller.submitPinForm}
/>
<CalibrationModal bind:open={calibrationOpen} />
<RadarCalibrationModal bind:open={radarCalibrationOpen} oncomplete={() => {
	if (navigation) void showNavigationOverlays();
}} />
