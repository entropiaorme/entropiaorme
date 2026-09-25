<script lang="ts">
	import type { TrackingLive, TrackingSnapshot } from '$lib/api';
	import OverlayStrip from '$lib/components/overlay/OverlayStrip.svelte';
	import { guideState } from '$lib/guide/state.svelte';
	import { ARMOUR_POPUP_WIDTH } from './guideDemoModel.svelte';

	let {
		demoTrackingLive,
		status,
		overlayStripPhase,
		demoOverlayVisible,
		demoArmourPopupVisible,
		demoArmourPopupRecorded,
		armourPopupTop,
		armourPopupLeft
	}: {
		demoTrackingLive: TrackingLive | null;
		status: TrackingSnapshot | null;
		overlayStripPhase: 'idle' | 'active';
		demoOverlayVisible: boolean;
		demoArmourPopupVisible: boolean;
		demoArmourPopupRecorded: boolean;
		armourPopupTop: number;
		armourPopupLeft: number;
	} = $props();
</script>

<!--
	Guide-only: inline overlay spawn. Mounts the real OverlayStrip with demo
	data routed through /demo/tracking/* so the actual overlay affordances
	render (TRACK button, mob row, stat pills, weapon, COST) instead of a
	static screenshot. Same fixed-positioning + flex-centring discipline as
	the character skill-scanner spawn (pointer-events-none so the guide
	click-blocker handles clicks). The cutout anchors on the strip's wrapper
	via data-guide-anchor; the surface module's anchor closure does a 2-phase
	priority cascade (this wrapper wins over the Overlay button once mounted).
-->
{#if guideState.isActive && demoTrackingLive}
	<!--
		Always-mounted slot for the demo overlay strip. Stays present for
		the whole duration of the dashboard guide (not just during the
		overlay-spawn step's visible-strip window) so the prose card's
		placementAnchor resolves immediately on step entry instead of
		falling back to viewport-pinned bottom-centre and snapping up
		when the strip mounts. Two anchor names cooperate:
		  - dashboard-overlay-spawn-slot: always present, for placement
		  - dashboard-overlay-spawn: only present when visible, for cutout
		so the cutout cascade still correctly stays on the Overlay button
		during Phase 1 of the looped play().
	-->
	<div class="fixed top-[350px] left-12 right-0 z-10 flex justify-center pointer-events-none">
		<span class="inline-flex" data-guide-anchor="dashboard-overlay-spawn-slot">
			<span
				class="inline-flex"
				style:opacity={demoOverlayVisible ? 1 : 0}
				data-guide-anchor={demoOverlayVisible ? 'dashboard-overlay-spawn' : undefined}
			>
				{#if overlayStripPhase === 'active'}
					<OverlayStrip data={demoTrackingLive} {status} />
				{:else}
					<!-- Idle synth: status='idle' + nulled session fields. Carries
						 the snapshot's trifectaAttribution + weaponAttribution
						 so the trifecta dropdown stays populated (the Calypso preset
						 reads as waiting-to-be-selected, not an em-dash placeholder).
						 status={null} passed to OverlayStrip so stat pills render as
						 em-dashes until tracking actually starts. -->
					<OverlayStrip
						data={{
							status: 'idle',
							weaponAttribution: demoTrackingLive.weaponAttribution,
							trifectaAttribution: demoTrackingLive.trifectaAttribution,
							repairOcrEnabled: demoTrackingLive.repairOcrEnabled,
							sessionName: 'Guide Demo',
							currentMob: null,
							currentTool: null
						}}
						status={null}
					/>
				{/if}
			</span>
		</span>
	</div>
{/if}

<!--
	Guide-only: fake protection-cost popup. Mirrors ProtectionCostPanel's
	unlimited repair step styled to match the OverlayStrip's
	glassmorphic look. Positioned dynamically below the strip's Cost
	button via syncArmourPopupPosition(). pointer-events-none on the
	wrapper so the live cursor never interacts with the stand-in;
	the virtual cursor uses clickRipple() rather than el.click() so the
	visual click does not dispatch a real event.
-->
{#if guideState.isActive && demoArmourPopupVisible}
	<div
		class="fixed z-10 pointer-events-none flex"
		style:left={`${armourPopupLeft}px`}
		style:top={`${armourPopupTop}px`}
		style:width={`${ARMOUR_POPUP_WIDTH}px`}
	>
		<div
			class="fake-armour-popup flex items-center justify-center gap-2 rounded-xl px-3 py-1.5 w-full"
			data-guide-anchor="overlay-armour-popup"
		>
			{#if demoArmourPopupRecorded}
				<span class="text-xs text-white/60 shrink-0">1.23 PED recorded</span>
				<span class="text-xs text-white/40 shrink-0">over 2 sessions</span>
			{:else}
				<span class="text-xs text-white/50 shrink-0">Unlimited repair:</span>
				<button
					type="button"
					class="fake-armour-record-btn"
					data-guide-anchor="overlay-armour-record-btn"
				>Scan Repair Terminal</button>
				<button type="button" class="fake-armour-manual-btn">Enter manually</button>
			{/if}
		</div>
	</div>
{/if}

<style>
	/* Guide-only: fake armour-cost popup. Glassmorphic palette matches the
	   OverlayStrip's .glass-panel rule (intentionally duplicated rather than
	   refactored to :global since the stand-in only lives in the dashboard
	   guide and never co-mounts with the real popup window). */
	.fake-armour-popup {
		background: rgba(10, 14, 23, 0.85);
		backdrop-filter: blur(16px) saturate(150%);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}
	.fake-armour-record-btn {
		padding: 3px 10px;
		border-radius: 4px;
		background: rgba(99, 179, 237, 0.18);
		border: 1px solid rgba(99, 179, 237, 0.42);
		color: rgb(125, 191, 240);
		font-size: 11px;
		font-weight: 500;
		line-height: 1;
	}
	.fake-armour-manual-btn {
		padding: 3px 10px;
		border-radius: 4px;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.16);
		color: rgba(255, 255, 255, 0.75);
		font-size: 11px;
		font-weight: 500;
		line-height: 1;
	}
</style>
