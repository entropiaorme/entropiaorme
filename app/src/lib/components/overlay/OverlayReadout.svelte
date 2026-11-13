<script lang="ts">
	// The tracking overlay: a layered readout headed by the session and its
	// figures, then the tool in hand and the mob, then what is running. Each
	// layer's control sits on the left rail beside the row it acts on.
	// The width follows the number of stats chosen, never the play: a name
	// longer than the block is cut with its full text on hover.
	import type { ConsumableDose, DetectedCreature, TrackingLive, TrackingStatus, WeaponMismatchDecision } from '$lib/api';
	import { enabledStats, overlayStats } from '$lib/statsCustomisation.svelte';
	import { getStatDef, type StatContext } from '$lib/statsRegistry';
	import { statsScope } from '$lib/statsScope.svelte';
	import { NO_DATA } from '$lib/utils/format';
	import SubWindowToggle from './SubWindowToggle.svelte';
	import type { ReadoutPanel } from '$lib/windows/overlayMenu';
	import ScreenUnreadNote from '$lib/features/tracking/ScreenUnreadNote.svelte';
	import type { DosesModel } from '$lib/features/consumables/dosesModel.svelte';
	import {
		describeEffects,
		doseState,
		formatCountdown,
		remainingSeconds,
	} from '$lib/features/consumables/doses';
	import {
		cueCopy,
		type HeldToolFeedback,
		type HeldToolTone,
		heldToolDisplay,
		SCREEN_SHOWS_TOOL,
		screenNotReadLine,
		screenShowsTool,
	} from '$lib/features/tracking/heldTool';
	import { primaryAction, standing } from '$lib/features/tracking/mobSource';
	import {
		createMobSourceModel,
		type MobSourceModel,
	} from '$lib/features/tracking/mobSourceModel.svelte';

	const noop = () => {};
	const settled = async () => {};

	let {
		data,
		status = null,
		statContext = {},
		toggling = false,
		releasing = false,
		selectingMob = false,
		savingDefinition = false,
		definitionEditable = true,
		savingActivity = false,
		activitiesMenuOpen = false,
		decidingWeapon = false,
		armourCostOpen = false,
		definitionMenuOpen = false,
		doses = null,
		dosesMenuOpen = false,
		equipmentMenuOpen = false,
		declaringEquipment = false,
		mobSource = null,
		mobQuery = $bindable(''),
		mobInput = $bindable(null),
		onStart = noop,
		onStop = noop,
		onReleaseMob = noop,
		onMobFocus = noop,
		onMobBlur = noop,
		onMobKeydown = noop,
		onDefinitionTrigger = noop,
		onActivitiesTrigger = noop,
		onWeaponDecision = noop,
		onArmourCostToggle = noop,
		onDosesTrigger = noop,
		onEquipmentTrigger = noop,
		openPanel = null,
		onPanelTrigger = noop,
	}: {
		data: TrackingLive;
		status?: TrackingStatus | null;
		/** What the markup stats read beyond the snapshot. */
		statContext?: StatContext;
		toggling?: boolean;
		releasing?: boolean;
		selectingMob?: boolean;
		savingDefinition?: boolean;
		definitionEditable?: boolean;
		savingActivity?: boolean;
		activitiesMenuOpen?: boolean;
		decidingWeapon?: boolean;
		armourCostOpen?: boolean;
		definitionMenuOpen?: boolean;
		/** The live dose readout, when this window shows one. */
		doses?: DosesModel | null;
		dosesMenuOpen?: boolean;
		equipmentMenuOpen?: boolean;
		declaringEquipment?: boolean;
		/** The mob control's state and writes; without one it is the
		 * typed mob alone. */
		mobSource?: MobSourceModel | null;
		mobQuery?: string;
		mobInput?: HTMLInputElement | null;
		onStart?: () => void | Promise<void>;
		onStop?: () => void | Promise<void>;
		onReleaseMob?: () => void | Promise<void>;
		onMobFocus?: () => void;
		onMobBlur?: () => void;
		onMobKeydown?: (event: KeyboardEvent) => void | Promise<void>;
		onDefinitionTrigger?: (anchor: HTMLButtonElement) => void | Promise<void>;
		onActivitiesTrigger?: (anchor: HTMLElement) => void | Promise<void>;
		onWeaponDecision?: (decision: WeaponMismatchDecision) => void | Promise<void>;
		onArmourCostToggle?: (event: MouseEvent) => void | Promise<void>;
		onDosesTrigger?: (anchor: HTMLElement) => void | Promise<void>;
		onEquipmentTrigger?: (anchor: HTMLElement) => void | Promise<void>;
		/** The panel whose sub-window is open, if any. */
		openPanel?: ReadoutPanel | null;
		/** A line that says more was clicked: open or close its sub-window,
		 * hung from `anchor`. */
		onPanelTrigger?: (panel: ReadoutPanel, anchor: HTMLElement) => void | Promise<void>;
	} = $props();

	const isActive = $derived(data.status === 'active');
	const inputAvailable = $derived(data.status === 'active' || data.status === 'idle');

	// --- The mob -------------------------------------------------
	const typedOnly = createMobSourceModel({
		view: () => data.mobSource ?? null,
		steerable: () => false,
		pin: settled,
		unpin: settled,
		setTyped: settled,
		clearTyped: settled,
		refresh: settled,
	});
	const mobModel = $derived(mobSource ?? typedOnly);
	const mobView = $derived(data.mobSource ?? null);
	const shown = $derived(standing(mobView));
	const mobAction = $derived(primaryAction(mobView));
	const mobSaving = $derived(mobModel.busy !== null);

	/** A mob as its species and what tells it apart within it. */
	function nameParts(creature: DetectedCreature): { species: string; rest: string } {
		const { name, species } = creature;
		if (species && name.startsWith(species) && name.length > species.length) {
			return { species, rest: name.slice(species.length).trim() };
		}
		return { species: name, rest: '' };
	}

	// --- The tool in hand ---------------------------------------------
	const activityLabel = $derived(
		data.currentActivity === 'treecutting'
			? 'Tree Cutting'
			: data.currentActivity === 'hunting'
				? 'Hunting'
				: null,
	);
	const keyless = $derived(data.keylessInput === true);
	const readingScreen = $derived(data.toolSource?.readingScreen === true);
	const toolScreen = $derived(heldToolDisplay(data.toolSource));
	const unreadLine = $derived(screenNotReadLine(data.toolSource));
	const cues = $derived(cueCopy(readingScreen));
	const weaponUndeclared = $derived(
		!readingScreen &&
			data.weaponDeclared === false &&
			(data.hotbarListenerActive === true || keyless) &&
			data.currentToolKind !== 'harvesting' &&
			(Boolean(data.currentTool) || (data.unpricedShots ?? 0) > 0),
	);
	const pickerRefused = $derived(screenShowsTool(data.toolSource));
	const hitCue: HeldToolFeedback | null = $derived(
		data.weaponHitCue
			? {
					text: data.weaponHitCue === 'boosted' ? 'Boosted' : 'Reduced',
					tone: data.weaponHitCue === 'boosted' ? 'tone-sky' : 'tone-dim',
					detail: cues.hit(data.weaponHitCue),
					testId: 'weapon-hit-cue',
				}
			: null,
	);
	// The small word beside a tool the screen shows: a hit cue first, then
	// the state's own marker, then the activity the tool implies.
	const screenCue = $derived.by((): HeldToolFeedback | null => {
		if (!toolScreen) return null;
		if (hitCue) return hitCue;
		if (toolScreen.marker) {
			return { text: toolScreen.marker, tone: 'tone-dim', detail: null, testId: 'held-tool-marker' };
		}
		if (activityLabel && (toolScreen.tone === 'normal' || toolScreen.tone === 'provisional')) {
			return {
				text: activityLabel,
				tone: 'tone-live',
				detail: `Held tool: recording as ${activityLabel}`,
				testId: 'activity-feedback',
			};
		}
		return null;
	});
	const fittedMarks = $derived(
		Array.from({ length: toolScreen?.setup?.fitted ?? 0 }, (_, index) => index),
	);
	const TOOL_TONES: Record<HeldToolTone, string> = {
		normal: 'tone-tool',
		provisional: 'tone-sky',
		muted: 'tone-dim',
		quiet: 'tone-faint',
		attention: 'tone-attention',
	};

	// --- The session's figures ----------------------------------------
	const lifetime = $derived(status?.lifetime ?? null);
	const showingLifetime = $derived(statsScope.current === 'lifetime' && lifetime !== null);
	const stats = $derived(enabledStats(overlayStats.current));
	// Two stats or fewer is the small form, for a small game window.
	const compact = $derived(stats.length <= 2);
	const SLOT_PX = 82;
	// Names may widen the block up to here; more stats than that widen it
	// themselves, so nothing is ever cut at the edge.
	const contentMax = $derived(Math.max(compact ? 420 : 520, stats.length * SLOT_PX));
	const contentMin = $derived(
		Math.max(compact ? 250 : 360, stats.length * SLOT_PX),
	);

	function formatElapsed(seconds: number): string {
		const h = Math.floor(seconds / 3600);
		const m = Math.floor((seconds % 3600) / 60);
		const s = seconds % 60;
		if (h > 0) return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
		return `${m}:${s.toString().padStart(2, '0')}`;
	}

	// --- What is running ----------------------------------------------
	const activities = $derived(data.activities ?? null);
	const standingActivities = $derived(activities?.active ?? []);
	const readyCount = $derived(activities?.readyCount ?? 0);
	let activitiesHandle = $state<HTMLButtonElement | null>(null);
	const dosesVisible = $derived(
		doses !== null &&
			isActive &&
			(doses.rows.length > 0 || doses.options.length > 0 || doses.undoable !== null),
	);

	function doseTitle(dose: ConsumableDose): string {
		const effects = describeEffects(dose.effects);
		const named = effects ? `${dose.itemName}: ${effects}` : dose.itemName;
		return dose.untimed ? `${named}, in effect until you end it` : named;
	}
</script>

{#snippet iconPin(filled: boolean)}
	<svg viewBox="0 0 24 24" fill={filled ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" stroke-linecap="round" aria-hidden="true">
		<path d="M9 3.5h6l-.8 5.2 3.3 3.3v2H6.5v-2l3.3-3.3z" /><path d="M12 14v6.5" fill="none" />
	</svg>
{/snippet}
{#snippet iconTool()}
	<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true">
		<circle cx="12" cy="12" r="6.5" /><circle cx="12" cy="12" r="1.2" fill="currentColor" /><path d="M12 2.5v4M12 17.5v4M2.5 12h4M17.5 12h4" />
	</svg>
{/snippet}
{#snippet iconFlag()}
	<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" stroke-linecap="round" aria-hidden="true">
		<path d="M5 21V4M5 4h11l-2 4 2 4H5" />
	</svg>
{/snippet}
{#snippet iconPill()}
	<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
		<rect x="3" y="8" width="18" height="8" rx="4" transform="rotate(-35 12 12)" /><path d="M9.4 8.2l5.2 7.6" />
	</svg>
{/snippet}
{#snippet iconShield()}
	<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true">
		<path d="M12 3l7.5 2.8v5.4c0 4.6-3 8.2-7.5 9.8-4.500-1.600-7.500-5.200-7.500-9.800V5.800z" />
	</svg>
{/snippet}
{#snippet iconCross()}
	<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
		<path d="M5.28 4.22a.75.75 0 0 0-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 1 0 1.06 1.06L8 9.06l2.72 2.72a.75.75 0 1 0 1.06-1.06L9.06 8l2.72-2.72a.75.75 0 0 0-1.06-1.06L8 6.94 5.28 4.22Z" />
	</svg>
{/snippet}

{#snippet mobName()}
	{#if shown.creature}
		{@const parts = nameParts(shown.creature)}
		<span class="mob-name" title={shown.creature.name}>
			<span class="species">{parts.species}</span>{#if parts.rest}{' '}<span class="maturity">{parts.rest}</span>{/if}
		</span>
	{:else}
		<span class="mob-name tone-faint">{NO_DATA}</span>
	{/if}
{/snippet}

{#snippet typedInput(placeholder: string, big: boolean)}
	<input
		bind:this={mobInput}
		class="mob-input {big ? 'mob-input-big' : ''}"
		bind:value={mobQuery}
		{placeholder}
		aria-label="Mob"
		disabled={selectingMob || mobSaving}
		onfocus={onMobFocus}
		onblur={onMobBlur}
		onkeydown={onMobKeydown}
	/>
{/snippet}

{#snippet toolCue(text: string, tone: string, detail: string | null, testId: string)}
	<span class="cue {tone}" title={detail ?? undefined} data-testid={testId}>{text}</span>
{/snippet}

{#snippet screenTool()}
	{#if toolScreen}
		<span
			class="tool-name {TOOL_TONES[toolScreen.tone]}"
			data-testid="held-tool-label"
			data-tone={toolScreen.tone}
			title={toolScreen.setup?.name ?? toolScreen.label}
		>
			{toolScreen.label}
		</span>
		{#if toolScreen.setup?.fitted}
			<span class="dots" aria-hidden="true" data-testid="held-tool-fitted" data-count={toolScreen.setup.fitted}>
				{#each fittedMarks as mark (mark)}<i></i>{/each}
			</span>
			<span class="sr-only">, {toolScreen.setup.fittedText}</span>
		{/if}
	{/if}
{/snippet}

<div
	class="readout"
	class:compact
	style="--content-min: {contentMin}px; --content-max: {contentMax}px; --slots: {Math.max(stats.length, 1)}; --slot: {SLOT_PX}px"
	data-overlay-surface
>
	<!-- ============ The session and its figures ============ -->
	<div class="rail">
		{#if isActive}
			<button type="button" class="stop" disabled={toggling} onclick={onStop} title="Stop tracking" aria-label="Stop tracking">
				{#if toggling}<span class="busy">...</span>{:else}<i></i>{/if}
			</button>
		{:else}
			<button type="button" class="track" disabled={toggling} onclick={onStart} title="Start tracking" aria-label="Start tracking">
				{#if toggling}
					<span class="busy">...</span>
				{:else}
					<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M6 3.5l15 8.5-15 8.5z" /></svg>
				{/if}
			</button>
		{/if}
	</div>
	<div class="cell title" data-testid="definition-facet">
		{#if isActive}
			<!-- Fixed while a session runs: it names the whole session, so it
				 is corrected on the session record afterwards. -->
			<span
				class="title-text {data.sessionName ? '' : 'tone-faint'}"
				title={data.sessionName
					? `${data.sessionName} (fixed for this session; correct it from the session record once it ends)`
					: undefined}
			>
				{data.sessionName ?? 'Session'}
			</span>
		{:else}
			<!-- The title is its own picker: plain text that opens the
				 sessions when tapped. -->
			<button
				type="button"
				class="title-text title-pick {definitionMenuOpen ? 'title-open' : ''} {data.sessionName ? '' : 'tone-faint'}"
				disabled={savingDefinition || !definitionEditable}
				aria-haspopup="menu"
				aria-expanded={definitionMenuOpen}
				title={data.sessionName ? `${data.sessionName}; pick the session for the next run` : 'Pick the session for the next run'}
				onclick={(event) => onDefinitionTrigger(event.currentTarget)}
			>
				{data.sessionName ?? 'Pick a session'}
			</button>
		{/if}
		{#if showingLifetime && lifetime}
			<span
				class="lifetime"
				data-testid="overlay-lifetime-marker"
				title={`Lifetime figures across ${lifetime.instanceCount} recorded ${lifetime.instanceCount === 1 ? 'session' : 'sessions'}. Change this on the dashboard.`}
			>
				Showing lifetime
			</span>
		{/if}
	</div>
	<div class="rail">
		{#if isActive}
			<!-- Always the live session's own elapsed, whatever scope the
				 figures read in. -->
			<span class="timer">{formatElapsed(data.elapsed ?? 0)}</span>
		{:else}
			<span class="track-word">Track</span>
		{/if}
	</div>
	<div class="cell stats">
		{#each stats as pref (pref.id)}
			{@const def = getStatDef(pref.id)}
			{#if def}
				{@const r = showingLifetime && lifetime ? def.renderLifetime(lifetime, statContext) : def.render(status, statContext)}
				{@const tone = r.value === NO_DATA ? 'tone-faint' : r.color === 'text-text' ? '' : r.color}
				<div class="stat">
					<b>{def.shortLabel ?? def.label}{#if r.qualifier}<span> · {r.qualifier}</span>{/if}</b>
					<em class={tone} title={r.incomplete}>
						{r.value}{#if r.incomplete}<sup aria-hidden="true">*</sup><span class="sr-only">, {r.incomplete}</span>{/if}
					</em>
				</div>
			{/if}
		{/each}
	</div>

	<div class="rule"></div>
	<!-- ============ In hand ============ -->
	<div class="rail">
		{#if data.weaponGuardrail}
			<span class="rail-word tone-attention">Firing</span>
		{:else if isActive}
			<button
				type="button"
				class="handle {equipmentMenuOpen ? 'handle-on' : ''}"
				data-testid="equipment-icon"
				aria-label="Declare equipment in hand"
				aria-expanded={equipmentMenuOpen}
				title={pickerRefused ? SCREEN_SHOWS_TOOL : 'Declare what is in hand: weapon, healer, or harvesting tool'}
				disabled={declaringEquipment || pickerRefused}
				onclick={(event) => onEquipmentTrigger(event.currentTarget)}
			>
				{@render iconTool()}
			</button>
		{:else}
			<span class="handle handle-inert" data-testid="equipment-icon" aria-hidden="true">{@render iconTool()}</span>
		{/if}
	</div>
	<div class="cell tool">
		{#if data.currentToolKind === 'healing' && !toolScreen}
			<span class="tool-name {data.currentTool ? 'tone-tool' : 'tone-faint'}" title={data.currentTool ?? undefined}>
				{data.currentTool || NO_DATA}
			</span>
		{:else if data.weaponGuardrail}
			<!-- The damage says another carried weapon is being fired than the
				 one declared. What is actually recorded reads in full; the
				 declared one gives way beside it, with the one decision. -->
			{@const cue = data.weaponGuardrail}
			<span
				class="fired"
				data-testid="weapon-guardrail-alert"
				title={cues.weaponMismatch(cue.recordingTool, cue.hotbarTool)}
			>
				{cue.recordingTool}
			</span>
			<span class="declared" title={cue.hotbarTool}>not {cue.hotbarTool}</span>
			<button
				type="button"
				class="decide decide-go"
				aria-label={`Confirm ${cue.recordingTool}`}
				title={cues.confirm(cue.recordingTool)}
				disabled={decidingWeapon}
				onclick={() => onWeaponDecision('confirm')}
			>
				Use this
			</button>
			<button
				type="button"
				class="decide"
				aria-label={`Keep ${cue.hotbarTool}`}
				title={`Keep ${cue.hotbarTool}: price these shots to it`}
				disabled={decidingWeapon}
				onclick={() => onWeaponDecision('keep')}
			>
				Keep
			</button>
		{:else if data.harvestGuardrail}
			<span
				class="fired"
				data-testid="guardrail-alert"
				title={cues.harvestMismatch(data.harvestGuardrail.expectedTool, data.harvestGuardrail.observedTool)}
			>
				{data.harvestGuardrail.expectedTool}
			</span>
			<span class="declared">not {data.harvestGuardrail.observedTool ?? 'no tool'}</span>
		{:else if toolScreen}
			<span class="facet-host" data-testid="held-tool-facet">
				<SubWindowToggle
					open={openPanel === 'heldTool'}
					title="The tool in hand: its set-up and what a use costs"
					testId="held-tool"
					onToggle={(anchor) => onPanelTrigger('heldTool', anchor)}
				>
					{@render screenTool()}
				</SubWindowToggle>
			</span>
			{#if screenCue}
				{@render toolCue(screenCue.text, screenCue.tone, screenCue.detail, screenCue.testId)}
			{/if}
		{:else}
			<span
				class="tool-name {data.currentTool ? (weaponUndeclared ? 'tone-dim' : 'tone-tool') : 'tone-faint'}"
				title={weaponUndeclared && data.currentTool ? `${data.currentTool}: inferred from damage, not declared` : (data.currentTool ?? undefined)}
			>
				{data.currentTool || (isActive ? NO_DATA : 'No tool in hand yet')}
			</span>
			{#if weaponUndeclared}
				{@render toolCue(
					keyless ? 'Pick weapon' : 'Press weapon slot',
					'tone-attention',
					keyless
						? 'No weapon declared: hits that fit several carried weapons go unpriced until you pick your weapon'
						: "No weapon declared: hits that fit several carried weapons go unpriced until you press your weapon's hotbar slot",
					'weapon-undeclared-cue',
				)}
			{:else if hitCue}
				{@render toolCue(hitCue.text, hitCue.tone, hitCue.detail, hitCue.testId)}
			{:else if activityLabel && data.currentTool}
				{@render toolCue(activityLabel, 'tone-live', `Held tool: recording as ${activityLabel}`, 'activity-feedback')}
			{/if}
		{/if}
		{#if unreadLine}
			<ScreenUnreadNote
				line={unreadLine}
				open={openPanel === 'screenUnread'}
				onToggle={(anchor) => onPanelTrigger('screenUnread', anchor)}
			/>
		{/if}
		<!-- Armour cost: recorded when repairing or scanning, so it needs no
			 running session. Quiet until reached for. -->
		<button
			type="button"
			class="handle armour {armourCostOpen ? 'handle-on' : ''}"
			aria-haspopup="dialog"
			aria-expanded={armourCostOpen}
			aria-label="Armour cost"
			title="Record an armour repair or a limited set's reading"
			onclick={onArmourCostToggle}
		>
			{@render iconShield()}
		</button>
	</div>

	<!-- ============ The mob ============ -->
	<div class="rail">
		{#if mobModel.steered}
			{#if mobAction === 'release'}
				<button
					type="button"
					class="handle handle-on"
					disabled={mobSaving}
					aria-label="Release the pinned mob"
					title="Pinned. Release it to go back to reading the mob off the screen"
					onclick={() => void mobModel.release()}
				>
					{@render iconPin(true)}
				</button>
			{:else}
				<button
					type="button"
					class="handle"
					disabled={mobSaving || mobAction !== 'pin'}
					aria-label="Pin this mob"
					title="Pin: every kill takes this mob until you release it"
					onclick={() => void mobModel.pinShown()}
				>
					{@render iconPin(false)}
				</button>
			{/if}
		{:else if mobView?.typed}
			<button
				type="button"
				class="handle"
				aria-label="Release mob"
				title="Clear the mob"
				onclick={onReleaseMob}
			>
				{#if releasing}<span class="busy">...</span>{:else}{@render iconCross()}{/if}
			</button>
		{/if}
	</div>
	<div class="cell mob-row" data-testid="mob-facet">
		{#if mobModel.steered}
			<span class="facet-host">
				<SubWindowToggle
					open={openPanel === 'mobSource'}
					title="Where the mob comes from"
					testId="mob-shown"
					data={{ standing: shown.kind }}
					onToggle={(anchor) => onPanelTrigger('mobSource', anchor)}
				>
					{@render mobName()}
				</SubWindowToggle>
			</span>
		{:else if inputAvailable && mobModel.inputShown}
			{@render typedInput('Type a mob...', true)}
		{:else}
			{@render mobName()}
		{/if}
	</div>

	<!-- ============ What is running ============ -->
	{#if activities?.visible || dosesVisible}
		<div class="rule"></div>
	{/if}
	{#if activities?.visible}
		<div class="rail">
			<button
				bind:this={activitiesHandle}
				type="button"
				class="handle {activitiesMenuOpen ? 'handle-on' : ''}"
				disabled={savingActivity}
				aria-haspopup="menu"
				aria-expanded={activitiesMenuOpen}
				aria-label="Activities"
				title="Declare what the play from now on counts toward"
				onclick={() => activitiesHandle && onActivitiesTrigger(activitiesHandle)}
			>
				{@render iconFlag()}
			</button>
		</div>
		<div class="cell running" data-testid="activities-facet">
			{#each standingActivities as activity (activity.key)}
				<button
					type="button"
					class="running-item"
					disabled={savingActivity}
					title={activity.handInWaiting
						? `Waiting for the next reward clump for ${activity.name}`
						: `Recording ${activity.name}; open the activities`}
					onclick={() => activitiesHandle && onActivitiesTrigger(activitiesHandle)}
				>
					<span class="truncate">{activity.name}</span>
					{#if activity.handInWaiting}<span class="tone-attention">waiting</span>{/if}
				</button>
			{:else}
				<span class="tone-dim">{readyCount > 0 ? `${readyCount} ready` : 'No activity declared'}</span>
			{/each}
		</div>
	{/if}
	{#if doses && dosesVisible}
		<div class="rail">
			<button
				type="button"
				class="handle {dosesMenuOpen ? 'handle-on' : ''}"
				aria-haspopup="menu"
				aria-expanded={dosesMenuOpen}
				aria-label="Take a dose"
				title="Take a dose of a consumable"
				disabled={doses.busy !== null || doses.options.length === 0}
				onclick={(event) => onDosesTrigger(event.currentTarget)}
			>
				{@render iconPill()}
			</button>
		</div>
		<div class="cell running" data-testid="overlay-doses">
			{#each doses.rows as dose (dose.id)}
				{@const state = doseState(dose, doses.now)}
				<span class="running-item dose {state === 'ended' ? 'tone-dim' : ''}" title={doseTitle(dose)} data-testid="overlay-dose">
					<span class="truncate">{dose.itemName}</span>
					{#if state === 'running' && dose.untimed}
						<button
							type="button"
							class="dose-act"
							aria-label={`End the effect of ${dose.itemName}`}
							title="It ran out: end its effect now"
							disabled={doses.busy !== null}
							onclick={() => void doses.end(dose)}
						>
							{@render iconCross()}
						</button>
					{:else if state === 'running'}
						<span class="countdown">{formatCountdown(remainingSeconds(dose, doses.now) ?? 0)}</span>
						<button
							type="button"
							class="dose-act"
							aria-label={`Remove this dose of ${dose.itemName}`}
							title="Remove this dose (a misclick): its effect and cost come off"
							disabled={doses.busy !== null}
							onclick={() => void doses.remove(dose)}
						>
							{@render iconCross()}
						</button>
					{:else}
						<span>ended</span>
						{#if dose.equipmentId !== null && doses.options.some((option) => option.equipmentId === dose.equipmentId)}
							<button
								type="button"
								class="panel-link"
								aria-label={`Take another dose of ${dose.itemName}`}
								disabled={doses.busy !== null}
								onclick={() => dose.equipmentId !== null && void doses.start(dose.equipmentId)}
							>
								Again
							</button>
						{/if}
					{/if}
				</span>
			{/each}
			{#if doses.undoable}
				{@const removed = doses.undoable.dose}
				<button
					type="button"
					class="running-item undo"
					title={`Put the dose of ${removed.itemName} back`}
					disabled={doses.busy !== null}
					onclick={() => void doses.restore(removed)}
				>
					Removed · Undo
				</button>
			{/if}
			{#if doses.rows.length === 0 && !doses.undoable}
				<span class="tone-dim">No dose running</span>
			{/if}
		</div>
	{/if}
</div>

<style>
	.readout {
		--rail: 48px;
		--pad: 13px;
		--t1: rgba(255, 255, 255, 0.95);
		--t2: rgba(255, 255, 255, 0.8);
		--t3: rgba(255, 255, 255, 0.58);
		--t4: rgba(255, 255, 255, 0.4);
		--hair: rgba(255, 255, 255, 0.075);
		--sky: #38bdf8;
		--live: #34d399;
		--amber: #fcd34d;
		--red: #f87171;
		position: relative;
		display: grid;
		grid-template-columns: var(--rail) minmax(var(--content-min), 1fr);
		width: max-content;
		max-width: calc(var(--rail) + var(--content-max));
		padding: 5px 0;
		border-radius: 9px;
		border: 1px solid rgba(255, 255, 255, 0.08);
		background:
			linear-gradient(90deg, rgba(255, 255, 255, 0.028) var(--rail), transparent var(--rail)),
			rgba(10, 14, 23, 0.95);
		backdrop-filter: blur(16px) saturate(150%);
		color: var(--t1);
		overflow: visible;
	}
	.readout.compact {
		--rail: 44px;
		--pad: 10px;
	}

	.rail {
		grid-column: 1;
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 18px;
	}
	.cell {
		grid-column: 2;
		display: flex;
		align-items: center;
		min-width: 0;
		padding: 0 var(--pad);
		white-space: nowrap;
	}

	/* ---- the session ---- */
	.title {
		height: 26px;
		gap: 10px;
		font-size: 17px;
		font-weight: 600;
		letter-spacing: -0.005em;
	}
	.title-text {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.title-pick {
		padding: 0;
		border: none;
		background: none;
		font: inherit;
		letter-spacing: inherit;
		color: var(--t1);
		text-align: left;
		cursor: pointer;
	}
	.title-pick:hover:not(:disabled),
	.title-pick:focus-visible,
	.title-open {
		color: var(--sky);
		outline: none;
	}
	.title-pick:disabled {
		cursor: default;
	}
	.lifetime {
		flex: none;
		margin-left: auto;
		font-size: 10px;
		font-weight: 400;
		letter-spacing: 0;
		color: var(--sky);
	}

	/* ---- in hand, and the mob ---- */
	.mob-row {
		height: 21px;
		font-size: 13px;
		font-weight: 500;
	}
	.mob-name {
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}
	.species {
		color: var(--t1);
	}
	.maturity {
		color: var(--sky);
	}
	.tool {
		height: 21px;
		gap: 8px;
		font-size: 13px;
		font-weight: 500;
	}
	.tool-name {
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}
	.compact .title {
		height: 23px;
		font-size: 14.5px;
	}
	.compact .mob-row {
		height: 19px;
		font-size: 12px;
	}
	.compact .tool {
		height: 19px;
		gap: 6px;
		font-size: 12px;
	}
	.dots {
		display: inline-flex;
		flex: none;
		gap: 3px;
	}
	.dots i {
		width: 4px;
		height: 4px;
		border-radius: 50%;
		background: var(--sky);
		opacity: 0.85;
	}
	.cue {
		flex: none;
		font-size: 11px;
		font-weight: 500;
	}
	.fired {
		flex: 0 1 auto;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		font-weight: 600;
		color: var(--amber);
	}
	/* Takes only the room left over: it never widens the block and gives
	   way before the weapon being recorded does. */
	.declared {
		flex: 1 1000 0;
		width: 0;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		font-size: 11px;
		font-weight: 400;
		color: var(--t3);
	}
	.decide {
		flex: none;
		height: 19px;
		padding: 0 8px;
		border-radius: 6px;
		border: 1px solid rgba(255, 255, 255, 0.16);
		background: rgba(255, 255, 255, 0.06);
		color: var(--t1);
		font-size: 11px;
		font-weight: 500;
		cursor: pointer;
	}
	.decide-go {
		border-color: rgba(252, 211, 77, 0.5);
		background: rgba(252, 211, 77, 0.14);
		color: var(--amber);
	}
	.decide:disabled {
		opacity: 0.4;
		cursor: default;
	}
	.armour {
		margin-left: auto;
		flex: none;
	}

	/* A name line that opens a sub-window: the toggle's own padding is
	   taken back so the name keeps the content edge. */
	.cell :global(.facet-host) {
		display: flex;
		align-items: center;
		min-width: 0;
		margin-left: -4px;
	}
	.cell :global(.facet-host .sub-toggle) {
		font-size: inherit;
		font-weight: inherit;
		min-width: 0;
		max-width: 100%;
	}

	/* ---- tones ---- */
	.tone-tool {
		color: var(--t2);
	}
	.tone-dim {
		color: var(--t3);
	}
	.tone-faint {
		color: var(--t4);
		font-weight: 400;
	}
	.tone-sky {
		color: var(--sky);
	}
	.tone-live {
		color: var(--live);
	}
	.tone-attention {
		color: var(--amber);
	}

	/* ---- hairlines ---- */
	.rule {
		grid-column: 2;
		height: 1px;
		margin: 4px var(--pad);
		background: var(--hair);
	}

	/* ---- figures ---- */
	.stats {
		display: grid;
		grid-template-columns: repeat(var(--slots), var(--slot));
		align-items: center;
		height: 36px;
	}
	.compact .stats {
		height: 34px;
	}
	.stat {
		display: flex;
		flex-direction: column;
		gap: 1px;
		min-width: 0;
	}
	.stat b {
		overflow: hidden;
		text-overflow: ellipsis;
		font-size: 10.5px;
		font-weight: 400;
		color: var(--t3);
	}
	.stat em {
		font-style: normal;
		font-size: 14.5px;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
		letter-spacing: -0.01em;
	}
	.stat sup {
		margin-left: 1px;
		font-size: 10px;
		font-weight: 400;
		color: var(--amber);
	}

	/* ---- what is running ---- */
	.running {
		min-height: 20px;
		gap: 16px;
		font-size: 11.5px;
		color: var(--t2);
	}
	.running-item {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
		padding: 0;
		border: none;
		background: none;
		color: inherit;
		font: inherit;
	}
	button.running-item {
		cursor: pointer;
	}
	button.running-item:hover:not(:disabled),
	button.running-item:focus-visible {
		color: var(--t1);
		outline: none;
	}
	.countdown {
		font-weight: 600;
		font-variant-numeric: tabular-nums;
		color: var(--t1);
	}
	/* A dose's own control is quiet until its dose is reached for. */
	.dose-act {
		display: grid;
		place-items: center;
		width: 16px;
		height: 16px;
		padding: 0;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--t4);
		opacity: 0.35;
		cursor: pointer;
	}
	.dose-act :global(svg) {
		width: 11px;
		height: 11px;
	}
	.dose:hover .dose-act,
	.dose-act:focus-visible {
		opacity: 1;
		background: rgba(255, 255, 255, 0.1);
		color: var(--t1);
		outline: none;
	}
	.undo {
		color: var(--sky);
		cursor: pointer;
	}

	/* ---- rail controls ---- */
	.handle {
		display: grid;
		place-items: center;
		width: 24px;
		height: 20px;
		padding: 0;
		border: none;
		border-radius: 5px;
		background: transparent;
		color: rgba(255, 255, 255, 0.5);
		cursor: pointer;
	}
	.handle :global(svg) {
		width: 14px;
		height: 14px;
	}
	.handle:hover:not(:disabled),
	.handle:focus-visible {
		background: rgba(255, 255, 255, 0.08);
		color: var(--t1);
		outline: none;
	}
	.handle-on {
		background: rgba(56, 189, 248, 0.14);
		color: var(--sky);
	}
	.handle:disabled {
		opacity: 0.35;
		cursor: default;
	}
	.handle-inert {
		opacity: 0.5;
		cursor: default;
	}
	.rail-word {
		font-size: 10px;
		font-weight: 600;
	}
	.stop,
	.track {
		display: grid;
		place-items: center;
		height: 17px;
		padding: 0;
		border-radius: 5px;
		cursor: pointer;
	}
	.stop {
		width: 22px;
		border: 1px solid rgba(248, 113, 113, 0.45);
		background: rgba(248, 113, 113, 0.13);
		color: var(--red);
	}
	.stop i {
		width: 7px;
		height: 7px;
		border-radius: 1.5px;
		background: var(--red);
	}
	.stop:hover:not(:disabled) {
		background: rgba(248, 113, 113, 0.24);
	}
	.track {
		width: 26px;
		border: 1px solid rgba(52, 211, 153, 0.5);
		background: rgba(52, 211, 153, 0.14);
		color: var(--live);
	}
	.track svg {
		width: 10px;
		height: 10px;
	}
	.track:hover:not(:disabled) {
		background: rgba(52, 211, 153, 0.26);
	}
	.stop:disabled,
	.track:disabled {
		opacity: 0.4;
		cursor: default;
	}
	.timer {
		font-size: 11px;
		font-weight: 600;
		font-variant-numeric: tabular-nums;
		letter-spacing: -0.01em;
		color: var(--live);
	}
	.track-word {
		font-size: 10.5px;
		font-weight: 600;
		color: var(--live);
	}
	.busy {
		font-size: 11px;
	}

	/* ---- typed mob ---- */
	.mob-input {
		width: 100%;
		padding: 2px 4px;
		border: none;
		border-bottom: 1px solid rgba(255, 255, 255, 0.1);
		background: transparent;
		color: var(--t1);
		font-size: 13px;
		font-weight: 400;
		outline: none;
	}
	.mob-input:focus {
		border-bottom-color: var(--sky);
	}
	.mob-input::placeholder {
		color: rgba(255, 255, 255, 0.25);
	}
	.mob-input-big {
		padding-left: 0;
		font-size: 13px;
		font-weight: 500;
	}

	.panel-link {
		padding: 0;
		border: none;
		background: none;
		font-size: 11px;
		font-weight: 400;
		color: var(--t3);
		cursor: pointer;
	}
	.panel-link:hover:not(:disabled) {
		color: var(--t1);
	}
</style>
