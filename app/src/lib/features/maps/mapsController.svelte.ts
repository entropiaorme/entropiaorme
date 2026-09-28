/**
 * The maps surface's pin-lifecycle controller: the pin form's transient state
 * (drop point, edit target, feedback) and the create/edit/delete/copy handlers
 * over the view model. Selection and viewport wiring stay in the surface
 * (`MapsSurface.svelte`); this keeps that wiring apart from the pin lifecycle.
 */

import { getNearbyMapPin, type MapPin } from '$lib/api';
import { describeError } from '$lib/view/errorState';
import type { GamePoint } from './coords';
import type { MapsModel } from './mapsModel.svelte';
import type { PinFormValues } from './PinEditModal.svelte';
import { formatWaypoint, type WaypointCopyResult } from './waypoint';

export function createMapsController(model: MapsModel) {
	let formOpen = $state(false);
	let dropPoint = $state<GamePoint>({ lon: 0, lat: 0 });
	let dropAltitude = $state<number | null>(null);
	let editingPin = $state<MapPin | null>(null);
	let feedback = $state<string | null>(null);
	let feedbackTimer: ReturnType<typeof setTimeout> | null = null;

	function flash(message: string) {
		feedback = message;
		if (feedbackTimer) clearTimeout(feedbackTimer);
		feedbackTimer = setTimeout(() => (feedback = null), 4000);
	}

	function openDropForm(point: GamePoint, altitude: number | null = null) {
		dropPoint = point;
		dropAltitude = altitude;
		editingPin = null;
		formOpen = true;
	}

	function openEditForm(pin: MapPin) {
		dropPoint = { lon: pin.lon, lat: pin.lat };
		editingPin = pin;
		formOpen = true;
	}

	async function submitPinForm(values: PinFormValues): Promise<boolean> {
		try {
			if (editingPin) {
				await model.editPin(editingPin.id, {
					name: values.name,
					icon: values.icon,
					kind: values.kind,
					radiusM: values.radiusM,
					notes: values.notes || null,
				});
				flash(`Pin "${values.name}" updated.`);
			} else if (model.selected) {
				const nearby = await getNearbyMapPin(
					model.selected.name,
					model.selectedViewId,
					dropPoint.lon,
					dropPoint.lat,
				);
				const allowNearby = nearby
					? window.confirm(
							`There is already a pin ("${nearby.pin.name}") ${nearby.distance.toFixed(1)} m away. Create another pin here anyway?`,
						)
					: false;
				if (nearby && !allowNearby) return false;
				await model.addPin({
					planet: model.selected.name,
					lon: dropPoint.lon,
					lat: dropPoint.lat,
					altitude: dropAltitude,
					name: values.name,
					icon: values.icon,
					kind: values.kind,
					radiusM: values.radiusM,
					notes: values.notes || null,
					sessionId: null,
					mapViewId: model.selectedViewId,
					pinConfigId: values.configId ?? null,
					allowNearby,
				});
				flash(`Pin "${values.name}" dropped.`);
			}
			return true;
		} catch (e) {
			flash(describeError(e, 'The pin could not be saved'));
			return false;
		}
	}

	async function deletePin(pin: MapPin) {
		if (!window.confirm(`Delete pin "${pin.name}"? This cannot be undone.`)) return;
		try {
			await model.removePin(pin.id);
			flash(`Pin "${pin.name}" deleted.`);
		} catch (e) {
			flash(describeError(e, 'The pin could not be deleted'));
		}
	}

	async function cooldownPin(pin: MapPin) {
		try {
			await model.cooldownPin(pin.id);
			flash(`"${pin.name}" put on cooldown.`);
		} catch (e) {
			flash(describeError(e, 'The tree could not be put on cooldown'));
		}
	}

	async function copyWaypoint(pin: MapPin): Promise<WaypointCopyResult> {
		const waypoint = formatWaypoint({
			technicalName: model.selected?.technicalName ?? null,
			lon: pin.lon,
			lat: pin.lat,
			altitude: pin.altitude,
			label: pin.name,
		});
		if (!waypoint) return { message: 'Waypoint unavailable', copied: false };
		try {
			await navigator.clipboard.writeText(waypoint);
			return { message: 'Waypoint copied.', copied: true };
		} catch {
			return { message: 'Copy failed', copied: false };
		}
	}

	return {
		get formOpen() {
			return formOpen;
		},
		set formOpen(value: boolean) {
			formOpen = value;
		},
		get dropPoint() {
			return dropPoint;
		},
		get editingPin() {
			return editingPin;
		},
		get feedback() {
			return feedback;
		},
		flash,
		openDropForm,
		openEditForm,
		submitPinForm,
		deletePin,
		cooldownPin,
		copyWaypoint,
	};
}

export type MapsController = ReturnType<typeof createMapsController>;
