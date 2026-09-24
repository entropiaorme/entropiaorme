<script lang="ts">
	import type { ProtectionOverview, TrackingLive, TrackingStatus } from '$lib/api';
	import { overlayStats, scopedStats } from '$lib/statsCustomisation.svelte';
	import { getStatDef } from '$lib/statsRegistry';
	import { statsScope } from '$lib/statsScope.svelte';
	import TrifectaSelector from './TrifectaSelector.svelte';
	import { ICON_EQUIPMENT, ICON_ARMOUR } from './icons';
	import { NO_DATA } from '$lib/utils/format';
	import { protectionCostAction } from '$lib/features/protection/protectionCostFlow';

	const noop = () => {};

	let {
		data,
		status = null,
		toggling = false,
		releasing = false,
		selectingMob = false,
		savingDefinition = false,
		definitionEditable = true,
		savingBoost = false,
		savingActivity = false,
		activitiesMenuOpen = false,
		trifectaSaving = false,
		armourCostOpen = false,
		armourSessionId = null,
		protection = null,
		protectionSaving = false,
		definitionMenuOpen = false,
		trifectaMenuOpen = false,
		mobQuery = $bindable(''),
		mobInput = $bindable(null),
		boostDraft = $bindable(''),
		inSessionArmourButton = $bindable(null),
		awaitingArmourTrackDecision = false,
		onStart = noop,
		onStop = noop,
		onArmourTrackDecision = noop,
		onReleaseMob = noop,
		onMobFocus = noop,
		onMobBlur = noop,
		onMobKeydown = noop,
		onDefinitionTrigger = noop,
		onBoostCommit = noop,
		onActivitiesTrigger = noop,
		onTrifectaTrigger = noop,
		onArmourCostToggle = noop,
		onProtectionSelect = noop
	}: {
		data: TrackingLive;
		status?: TrackingStatus | null;
		toggling?: boolean;
		releasing?: boolean;
		selectingMob?: boolean;
		savingDefinition?: boolean;
		definitionEditable?: boolean;
		savingBoost?: boolean;
		savingActivity?: boolean;
		activitiesMenuOpen?: boolean;
		trifectaSaving?: boolean;
		armourCostOpen?: boolean;
		armourSessionId?: string | null;
		protection?: ProtectionOverview | null;
		protectionSaving?: boolean;
		definitionMenuOpen?: boolean;
		trifectaMenuOpen?: boolean;
		mobQuery?: string;
		mobInput?: HTMLInputElement | null;
		boostDraft?: string;
		inSessionArmourButton?: HTMLButtonElement | null;
		awaitingArmourTrackDecision?: boolean;
		onStart?: () => void | Promise<void>;
		onStop?: () => void | Promise<void>;
		onArmourTrackDecision?: (action: 'yes' | 'no') => void | Promise<void>;
		onReleaseMob?: () => void | Promise<void>;
		onMobFocus?: () => void;
		onMobBlur?: () => void;
		onMobKeydown?: (event: KeyboardEvent) => void | Promise<void>;
		onDefinitionTrigger?: (anchor: HTMLButtonElement) => void | Promise<void>;
		onBoostCommit?: () => void | Promise<void>;
		onActivitiesTrigger?: (anchor: HTMLElement) => void | Promise<void>;
		onTrifectaTrigger?: (anchor: HTMLButtonElement) => void | Promise<void>;
		onArmourCostToggle?: (event: MouseEvent) => void | Promise<void>;
		onProtectionSelect?: (id: string) => void | Promise<void>;
	} = $props();

	// The Activities menu's anchor: the section, which survives the chip
	// churn a declaration causes (see the markup below).
	let activitiesSection = $state<HTMLDivElement | null>(null);

	const isTrifectaAttribution = $derived(data.weaponAttribution === 'trifecta');
	const isActive = $derived(data.status === 'active');
	// The declared mob is the kill-stamp source and may change mid-session,
	// so its input is available in both states; a standing declaration
	// shows as a label with a release control beside it.
	const showManualInput = $derived(
		(data.status === 'active' || data.status === 'idle') && !data.currentMob
	);
	// The session (and the name it writes) is session-grain: picked
	// before a session, fixed while one runs, corrected afterwards on the
	// session record. (The boost is the other way round: it stamps each
	// skill gain, so it stays editable throughout.)
	// The Activities readout, straight off the tracking frame: whether
	// the control appears at all, what is standing, and how many rows a
	// tap could start. The menu's own rows are fetched when it opens.
	// The instance/family scope, owned by the dashboard and followed
	// here. The lifetime block is absent when the session belongs to no
	// definition, so the strip falls back to the instance rather than
	// drawing figures it has no family to fill.
	const lifetime = $derived(status?.lifetime ?? null);
	const showingLifetime = $derived(statsScope.current === 'lifetime' && lifetime !== null);
	const overlayScope = $derived(showingLifetime ? 'lifetime' : 'instance');
	const activities = $derived(data.activities ?? null);
	const standing = $derived(activities?.active ?? []);
	const readyCount = $derived(activities?.readyCount ?? 0);
	// What the held tool implies the next action records as. Derived from
	// evidence, never declared, so it is shown as feedback and never asked.
	const activityLabel = $derived(
		data.currentActivity === 'treecutting'
			? 'Tree Cutting'
			: data.currentActivity === 'hunting'
				? 'Hunting'
				: null
	);
	const activeProtection = $derived(
		protection?.loadouts.find((loadout) => loadout.id === protection?.activeLoadoutId) ?? null
	);
	const enabledPills = $derived(
		scopedStats(overlayStats.current, overlayScope, { fallback: false }),
	);
	const costAction = $derived(
		protectionCostAction(protection, data.trackProtectionBySegment !== false),
	);

	function formatElapsed(seconds: number): string {
		const h = Math.floor(seconds / 3600);
		const m = Math.floor((seconds % 3600) / 60);
		const s = seconds % 60;
		if (h > 0) return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
		return `${m}:${s.toString().padStart(2, '0')}`;
	}
</script>

<!-- Glassmorphic container -->
<div class="overlay-strip glass-panel flex items-center gap-3 rounded-xl px-4 py-2 w-max">
	<!-- Track Button + Timer -->
	<div class="flex items-center gap-3 shrink-0 border-r border-white/10 pr-3">
		{#if awaitingArmourTrackDecision && data.status === 'active'}
			<div class="armour-prompt flex items-center gap-1.5 shrink-0">
				<span class="text-[10px] font-semibold text-amber-300 tracking-wide whitespace-nowrap">Record armour costs?</span>
				<button
					type="button"
					class="armour-prompt-btn armour-prompt-yes"
					disabled={toggling}
					onclick={() => onArmourTrackDecision('yes')}
				>Record</button>
				<button
					type="button"
					class="armour-prompt-btn armour-prompt-no"
					disabled={toggling}
					onclick={() => onArmourTrackDecision('no')}
				>Later</button>
			</div>
		{:else}
			<button
				class={data.status === 'active' ? 'stop-btn' : 'start-btn'}
				disabled={toggling}
				onclick={data.status === 'active' ? onStop : onStart}
				title={data.status === 'active' ? 'Stop tracking' : 'Start tracking'}
			>
				{#if toggling}
					<span class="text-[10px] px-1">...</span>
				{:else if data.status === 'active'}
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="w-2.5 h-2.5">
						<rect x="3" y="3" width="10" height="10" rx="1" />
					</svg>
				{:else}
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="w-3 h-3">
						<path d="M4 3.5a.5.5 0 0 1 .757-.429l8 4.8a.5.5 0 0 1 0 .858l-8 4.8A.5.5 0 0 1 4 13V3.5z" />
					</svg>
					<span class="font-bold tracking-wide">TRACK</span>
				{/if}
			</button>
		{/if}
		{#if data.status === 'active'}
			<div class="flex items-center gap-1.5">
				<span class="relative flex h-2 w-2 shrink-0">
					<span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75"></span>
					<span class="relative inline-flex rounded-full h-2 w-2 bg-emerald-400"></span>
				</span>
				<!-- Always the live session's own elapsed, whatever scope
					 the pills read in: this readout sits under a pulsing
					 live cue, so it must be the thing that is actually
					 ticking. The family's summed duration is a figure,
					 and figures live in the labelled pill group. -->
				<span class="text-sm font-semibold text-emerald-400 tabular-nums tracking-wider w-12 text-center">
					{formatElapsed(data.elapsed ?? 0)}
				</span>
			</div>
		{/if}
	</div>

	<!-- Session facets: the independent, co-recorded attributions a
		 session carries. Each control here declares gameplay from now on,
		 so a facet is editable while a session runs only if its stamp is
		 finer-grained than the session. The boost is (it stamps each skill
		 gain, so a pill expiring is recordable); the name is not (it names
		 the whole session, so a live edit could only rewrite history) and
		 is corrected on the session record instead. -->
	<div
		class="flex items-center gap-2 shrink-0 border-r border-white/10 pr-3"
		data-guide-anchor="overlay-session-section"
	>
		<div class="w-32 flex flex-col shrink-0">
			<span class="facet-label">Session</span>
			<div class="flex items-center gap-1" data-testid="definition-facet">
				{#if isActive && data.sessionName}
					<div
						class="text-sm font-medium text-white/90 truncate px-1 min-w-0 flex-1"
						title={`${data.sessionName} (fixed for this session; correct it from the session record once it ends)`}
					>
						{data.sessionName}
					</div>
				{:else}
					<button
						type="button"
						class="facet-chip min-w-0 flex-1 {data.sessionName ? 'facet-chip-open' : ''}"
						disabled={savingDefinition || !definitionEditable}
						aria-haspopup="menu"
						aria-expanded={definitionMenuOpen}
						title={!definitionEditable
							? 'The session is fixed while one runs'
							: data.sessionName
								? `${data.sessionName}; pick the session for the next run`
								: 'Pick the session for the next run'}
						onclick={(event) => onDefinitionTrigger(event.currentTarget as HTMLButtonElement)}
					>
						{#if data.sessionName}
							<span class="truncate">{data.sessionName}</span>
						{:else}
							<span class="text-white/40">Pick...</span>
						{/if}
					</button>
					<!-- No clear: a session always runs under a definition, so
						 "nothing in particular" is picked from the menu (the
						 protected default) rather than emptied here. -->
				{/if}
			</div>
		</div>

		<!-- Skill boost: the labelled percentage of the pill in force,
			 because it changes how PES reads. Three declarations, not
			 two: blank claims nothing, a typed 0 declares deliberately
			 unboosted play (the baseline a boost's effect is measured
			 against), and a number declares its magnitude. Editable at
			 any time: re-declaring when a pill runs out marks every gain
			 from that moment onward, and never touches the ones already
			 stamped. -->
		<div class="flex flex-col shrink-0">
			<span class="facet-label">Boost</span>
			<div class="flex items-baseline">
				<input
					class="w-9 bg-transparent border-b border-white/10 focus:border-accent text-sm text-white/90 px-1 py-0.5 outline-none placeholder:text-white/20 tabular-nums transition-colors"
					bind:value={boostDraft}
					placeholder={NO_DATA}
					inputmode="numeric"
					aria-label="Skill boost percent"
					title="Boost percent in force. Leave blank to claim nothing; enter 0 to record deliberately unboosted play."
					disabled={savingBoost}
					onblur={onBoostCommit}
					onkeydown={(event) => {
						if (event.key === 'Enter') {
							event.preventDefault();
							void onBoostCommit();
						}
					}}
				/>
				<span class="text-[10px] text-white/30 leading-none">%</span>
			</div>
		</div>

		<!-- Activities: what the play from now on counts toward. One
			 control over the session's authored roster and whatever the
			 mission log actually carries, so switching from one boss to
			 the next is a single tap. Absent, not disabled, when the
			 session has nothing to offer: a deliberately simple session
			 gets no activity surface at all. -->
		{#if activities?.visible}
			<!-- The section element is the menu's anchor, not the chip
				 clicked: declaring something swaps the ready-count button
				 for chips, so a button anchor would be destroyed by the
				 very action that needs to re-present the menu over it. -->
			<div
				class="flex flex-col shrink-0"
				data-testid="activities-facet"
				bind:this={activitiesSection}
			>
				<span class="facet-label">Activities</span>
				<div class="flex items-center gap-1">
					{#if standing.length > 0}
						{#each standing as activity (activity.key)}
							<button
								type="button"
								class="facet-chip facet-chip-open max-w-[140px]"
								disabled={savingActivity}
								aria-haspopup="menu"
								aria-expanded={activitiesMenuOpen}
								title={activity.handInWaiting
									? `Waiting for the next reward clump for ${activity.name}`
									: `Recording ${activity.name}; open the activities`}
								onclick={() => activitiesSection && onActivitiesTrigger(activitiesSection)}
							>
								<span class="truncate">{activity.name}</span>
								{#if activity.handInWaiting}
									<span class="text-[9px] font-semibold text-sky-200/80">Waiting</span>
								{/if}
							</button>
						{/each}
					{:else}
						<button
							type="button"
							class="facet-chip"
							disabled={savingActivity}
							aria-haspopup="menu"
							aria-expanded={activitiesMenuOpen}
							title="Declare what the play from now on counts toward"
							onclick={() => activitiesSection && onActivitiesTrigger(activitiesSection)}
						>
							{#if readyCount > 0}
								<span class="whitespace-nowrap">{readyCount} ready</span>
							{:else}
								<span>{NO_DATA}</span>
							{/if}
						</button>
					{/if}
				</div>
			</div>
		{/if}
	</div>

	<!-- Declared mob: the source of each kill's mob stamp, changeable
		 mid-session (an off-declaration kill still stamps the declared
		 mob until detection can read the target directly). -->
	<div
		class="flex items-center gap-2 shrink-0 border-r border-white/10 pr-3"
		data-guide-anchor="overlay-mob-section"
	>
		<div class="w-32 flex flex-col shrink-0">
			<span class="facet-label">Mob</span>
			<div class="flex items-center">
				{#if showManualInput}
					<input
						bind:this={mobInput}
						class="w-full bg-transparent border-b border-white/10 focus:border-accent text-sm text-white/90 px-1 py-0.5 outline-none placeholder:text-white/20 transition-colors"
						bind:value={mobQuery}
						placeholder="Mob..."
						disabled={selectingMob}
						onfocus={onMobFocus}
						onblur={onMobBlur}
						onkeydown={onMobKeydown}
					/>
				{:else if data.currentMob}
					<div class="text-sm font-medium text-white/90 truncate px-1 w-full">{data.currentMob}</div>
				{:else}
					<div class="text-sm font-medium text-white/20 px-1">{NO_DATA}</div>
				{/if}
			</div>
		</div>
		{#if data.currentMob}
			<button
				type="button"
				class="release-btn shrink-0"
				aria-label="Release mob"
				onclick={onReleaseMob}
				title="Release mob"
			>
				{releasing ? '...' : 'x'}
			</button>
		{/if}
	</div>

	<!-- Trifecta/Weapon Section. No own separator; the adjacent armour section
		 owns the boundary via its left border. -->
	<div
		class="flex items-center gap-2 shrink-0"
		data-guide-anchor="overlay-equipment-section"
	>
		<span class="text-white/40 shrink-0">{@html ICON_EQUIPMENT}</span>
		{#if data.currentToolKind === 'healing'}
			<div class="text-xs {data.currentTool ? 'text-white/70' : 'text-white/20'} truncate max-w-[120px]">
				{data.currentTool || NO_DATA}
			</div>
		{:else if isTrifectaAttribution}
			<TrifectaSelector
				trifecta={data.trifectaAttribution}
				tone={data.status === 'active' ? 'active' : 'idle'}
				menuOpen={trifectaMenuOpen}
				disabled={trifectaSaving}
				ontrigger={onTrifectaTrigger}
			/>
		{:else if data.harvestGuardrail}
			<!-- The guardrail cue: loot evidence disagrees with the hotbar's
				 tool. The believed tool shows in red (the questionable
				 belief) with the corrected attribution beneath, until a
				 hotbar press or agreeing loot resolves it. -->
			<div
				class="flex flex-col min-w-0"
				title={`Board output says ${data.harvestGuardrail.expectedTool}; hotbar shows ${data.harvestGuardrail.observedTool ?? 'no tool'}`}
				data-testid="guardrail-alert"
			>
				<div class="text-xs text-red-400 animate-pulse truncate max-w-[120px]">
					{data.harvestGuardrail.observedTool ?? 'No tool'}
				</div>
				<!-- Never truncated: what is actually being recorded must be
					 readable in full, so the self-sizing window widens for it. -->
				<div class="text-[10px] leading-tight text-white/70 whitespace-nowrap">
					Recording: {data.harvestGuardrail.expectedTool}
				</div>
			</div>
		{:else}
			<div class="flex flex-col min-w-0">
				<div class="text-xs {data.currentTool ? 'text-white/70' : 'text-white/20'} truncate max-w-[120px]">
					{data.currentTool || NO_DATA}
				</div>
				{#if activityLabel}
					<!-- Derived, never declared: the held tool implies which
						 activity the next action records as. What actually gets
						 recorded still follows the loot evidence, so this reads
						 as feedback the user can catch disagreeing. -->
					<div
						class="text-[9px] leading-tight uppercase tracking-wider text-white/35 whitespace-nowrap"
						title="Held tool: recording as {activityLabel}"
						data-testid="activity-feedback"
					>
						{activityLabel}
					</div>
				{/if}
			</div>
		{/if}
	</div>

	<!-- Active protection identity and live selection. -->
	{#if data.trackProtectionCosts !== false && data.trackProtectionBySegment !== false && protection && protection.loadouts.length > 0}
		<div class="flex flex-col shrink-0 border-l border-white/10 pl-3" data-testid="protection-facet">
			<span class="facet-label">Armour</span>
			{#if protection.loadouts.length === 1}
				<div class="px-1 text-xs text-white/70 whitespace-nowrap" title="Armour recorded from now on">
					{activeProtection?.name ?? protection.loadouts[0].name}
				</div>
			{:else}
				<div class="flex items-center gap-1">
					{#each protection.loadouts as loadout (loadout.id)}
						<button
							type="button"
							class="facet-chip max-w-[130px] {loadout.id === protection.activeLoadoutId ? 'facet-chip-open' : ''}"
							disabled={protectionSaving}
							aria-pressed={loadout.id === protection.activeLoadoutId}
							title={`Record ${loadout.name} from now on`}
							onclick={() => onProtectionSelect(loadout.id)}
						>
							<span class="truncate">{loadout.name}</span>
						</button>
					{/each}
				</div>
			{/if}
		</div>
	{/if}

	<!-- Armour cost, sequenced from the active loadout. -->
	{#if data.trackProtectionCosts !== false}
		<div
			class="flex items-center gap-2 shrink-0 border-l border-white/10 pl-3"
			data-guide-anchor="overlay-armour-section"
		>
			<span class="text-white/40 shrink-0">{@html ICON_ARMOUR}</span>
			<button
				class="px-2 py-0.5 rounded-[4px] border text-[9px] font-medium transition-all
					{armourSessionId && costAction.enabled
						? armourCostOpen
							? 'cursor-pointer bg-accent/20 border-accent/40 text-accent'
							: 'cursor-pointer bg-white/5 border-white/10 text-white/60 hover:bg-white/10 hover:text-white/90'
						: 'cursor-not-allowed bg-white/5 border-white/10 text-white/20'}"
				bind:this={inSessionArmourButton}
				disabled={!armourSessionId || !costAction.enabled}
				aria-haspopup="dialog"
				aria-expanded={armourCostOpen}
				onclick={onArmourCostToggle}
				title={armourSessionId ? costAction.label : 'Start or stop a session to enable'}
				data-guide-anchor="overlay-armour-cost-btn"
			>
				Cost
			</button>
		</div>
	{/if}

	<!-- Customisable stat pills (driven by the overlay stat prefs): treated as
		 one unit, so the section separator sits at the unit boundary, not
		 between individual pills. -->
	{#if enabledPills.length > 0}
		<div class="flex items-center gap-4 shrink-0 border-l border-white/10 pl-3">
			<!-- The strip carries no scope CONTROL of its own: it
				 follows the dashboard's choice, so the flip is a
				 deliberate trip there rather than another control
				 competing for width here. It does carry a scope
				 MARKER, because the pills below are labelled
				 identically in either scope: without it, a family
				 total would sit in the slot an instance figure
				 usually occupies with nothing saying so. -->
			{#if showingLifetime && lifetime}
				<div
					class="flex flex-col items-center justify-center gap-0.5 shrink-0"
					data-testid="overlay-lifetime-marker"
					title={`Lifetime figures across ${lifetime.instanceCount} recorded ${lifetime.instanceCount === 1 ? 'session' : 'sessions'}. Change this on the dashboard.`}
				>
					<span class="text-[10px] font-bold text-white/40 tracking-wider uppercase leading-none">Showing</span>
					<span class="text-sm font-semibold leading-none text-amber-300/90">Lifetime</span>
				</div>
			{/if}
			{#each enabledPills as pref (pref.id)}
				{@const def = getStatDef(pref.id)}
				{#if def}
					{@const r = showingLifetime && def.renderLifetime && lifetime
						? def.renderLifetime(lifetime)
						: def.render(status)}
					{@const valueColor = r.value === NO_DATA
						? 'text-white/25'
						: r.color === 'text-text'
							? 'text-white/85'
							: r.color}
					<div class="flex flex-col items-center justify-center gap-0.5 shrink-0">
						<span class="text-[10px] font-bold text-white/40 tracking-wider uppercase leading-none">{def.shortLabel ?? def.label}</span>
						<span class="text-sm font-semibold tabular-nums leading-none {valueColor}">{r.value}</span>
					</div>
				{/if}
			{/each}
		</div>
	{/if}
</div>

<style>
	.overlay-strip {
		overflow: visible;
	}

	.glass-panel {
		background: rgba(10, 14, 23, 0.85);
		backdrop-filter: blur(16px) saturate(150%);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}

	.facet-label {
		font-size: 9px;
		font-weight: 700;
		line-height: 1;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: rgba(255, 255, 255, 0.3);
		padding-left: 4px;
		margin-bottom: 2px;
	}

	.facet-chip {
		display: flex;
		align-items: center;
		padding: 2px 8px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.7);
		font-size: 11px;
		line-height: 1.35;
		cursor: pointer;
		transition: all 150ms ease-out;
	}
	.facet-chip:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.1);
		color: rgba(255, 255, 255, 0.9);
		border-color: rgba(255, 255, 255, 0.25);
	}
	.facet-chip-open {
		background: rgba(56, 189, 248, 0.2);
		border-color: rgba(56, 189, 248, 0.4);
		color: rgb(125, 211, 252);
	}
	.facet-chip:disabled {
		opacity: 0.35;
		cursor: default;
	}

	.release-btn {
		width: 18px;
		height: 18px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.15);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.4);
		font-size: 10px;
		line-height: 1;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: all 150ms ease-out;
	}
	.release-btn:hover {
		background: rgba(255, 255, 255, 0.1);
		color: rgba(255, 255, 255, 0.7);
		border-color: rgba(255, 255, 255, 0.25);
	}
	.release-btn:disabled {
		opacity: 0.3;
		cursor: default;
	}

	.start-btn {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 4px 10px;
		border-radius: 5px;
		border: 1px solid rgba(52, 211, 153, 0.3);
		background: rgba(52, 211, 153, 0.1);
		color: rgba(52, 211, 153, 0.9);
		font-size: 11px;
		font-weight: 500;
		cursor: pointer;
		transition: all 150ms ease-out;
	}
	.start-btn:hover {
		background: rgba(52, 211, 153, 0.2);
		border-color: rgba(52, 211, 153, 0.5);
	}
	.start-btn:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.stop-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		border-radius: 4px;
		border: 1px solid rgba(248, 113, 113, 0.3);
		background: rgba(248, 113, 113, 0.1);
		color: rgba(248, 113, 113, 0.8);
		cursor: pointer;
		transition: all 150ms ease-out;
	}
	.stop-btn:hover {
		background: rgba(248, 113, 113, 0.2);
		border-color: rgba(248, 113, 113, 0.5);
		color: rgba(248, 113, 113, 1);
	}
	.stop-btn:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.armour-prompt {
		padding: 3px 8px;
		border-radius: 5px;
		border: 1px solid rgba(251, 191, 36, 0.4);
		background: rgba(251, 191, 36, 0.12);
	}
	.armour-prompt-btn {
		padding: 2px 8px;
		border-radius: 4px;
		font-size: 10px;
		font-weight: 600;
		line-height: 1;
		border: 1px solid transparent;
		cursor: pointer;
		transition: background 120ms ease, border-color 120ms ease;
	}
	.armour-prompt-yes {
		background: rgba(251, 191, 36, 0.22);
		border-color: rgba(251, 191, 36, 0.5);
		color: rgb(253, 224, 71);
	}
	.armour-prompt-yes:hover {
		background: rgba(251, 191, 36, 0.32);
		border-color: rgba(251, 191, 36, 0.7);
	}
	.armour-prompt-no {
		background: rgba(255, 255, 255, 0.06);
		border-color: rgba(255, 255, 255, 0.18);
		color: rgba(255, 255, 255, 0.75);
	}
	.armour-prompt-no:hover {
		background: rgba(255, 255, 255, 0.12);
		border-color: rgba(255, 255, 255, 0.3);
	}
	.armour-prompt-btn:disabled {
		opacity: 0.4;
		cursor: default;
	}
</style>
