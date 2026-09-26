<script lang="ts">
	import type { TrackingLive, TrackingStatus, WeaponMismatchDecision } from '$lib/api';
	import { overlayStats, scopedStats } from '$lib/statsCustomisation.svelte';
	import { getStatDef } from '$lib/statsRegistry';
	import { statsScope } from '$lib/statsScope.svelte';
	import { ICON_EQUIPMENT, ICON_ARMOUR } from './icons';
	import OverlayDoses from '$lib/features/consumables/OverlayDoses.svelte';
	import type { DosesModel } from '$lib/features/consumables/dosesModel.svelte';
	import { NO_DATA } from '$lib/utils/format';

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
		decidingWeapon = false,
		armourCostOpen = false,
		definitionMenuOpen = false,
		doses = null,
		dosesMenuOpen = false,
		mobQuery = $bindable(''),
		mobInput = $bindable(null),
		boostDraft = $bindable(''),
		onStart = noop,
		onStop = noop,
		onReleaseMob = noop,
		onMobFocus = noop,
		onMobBlur = noop,
		onMobKeydown = noop,
		onDefinitionTrigger = noop,
		onBoostCommit = noop,
		onActivitiesTrigger = noop,
		onWeaponDecision = noop,
		onArmourCostToggle = noop,
		onDosesTrigger = noop
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
		decidingWeapon?: boolean;
		armourCostOpen?: boolean;
		definitionMenuOpen?: boolean;
		/** The live dose readout, when this window shows one. */
		doses?: DosesModel | null;
		dosesMenuOpen?: boolean;
		mobQuery?: string;
		mobInput?: HTMLInputElement | null;
		boostDraft?: string;
		onStart?: () => void | Promise<void>;
		onStop?: () => void | Promise<void>;
		onReleaseMob?: () => void | Promise<void>;
		onMobFocus?: () => void;
		onMobBlur?: () => void;
		onMobKeydown?: (event: KeyboardEvent) => void | Promise<void>;
		onDefinitionTrigger?: (anchor: HTMLButtonElement) => void | Promise<void>;
		onBoostCommit?: () => void | Promise<void>;
		onActivitiesTrigger?: (anchor: HTMLElement) => void | Promise<void>;
		onWeaponDecision?: (decision: WeaponMismatchDecision) => void | Promise<void>;
		onArmourCostToggle?: (event: MouseEvent) => void | Promise<void>;
		onDosesTrigger?: (anchor: HTMLElement) => void | Promise<void>;
	} = $props();

	// The Activities menu's anchor: the section, which survives the chip
	// churn a declaration causes (see the markup below).
	let activitiesSection = $state<HTMLDivElement | null>(null);

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
	const enabledPills = $derived(
		scopedStats(overlayStats.current, overlayScope, { fallback: false }),
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

	<!-- Weapon section. No own separator; the adjacent armour section
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
		{:else if data.weaponGuardrail}
			<!-- The weapon cue: the damage says another carried weapon is
				 being fired than the one the hotbar declared. The declared
				 weapon shows in red (the questionable belief) with what is
				 actually being recorded beneath, and the one decision the
				 player can make without leaving the game: confirm the switch
				 (record it from now, and reprice the shots since the last
				 press it plausibly fired) or keep the hotbar's weapon. -->
			{@const cue = data.weaponGuardrail}
			<div class="flex items-center gap-1.5" data-testid="weapon-guardrail-alert">
				<div
					class="flex flex-col min-w-0"
					title={`Damage fits ${cue.recordingTool}; the hotbar shows ${cue.hotbarTool}`}
				>
					<div class="text-xs text-red-400 animate-pulse truncate max-w-[120px]">
						{cue.hotbarTool}
					</div>
					<!-- Never truncated: what is actually being recorded must be
						 readable in full, so the self-sizing window widens for it. -->
					<div class="text-[10px] leading-tight text-white/70 whitespace-nowrap">
						Recording: {cue.recordingTool}
					</div>
				</div>
				<button
					type="button"
					class="cue-btn cue-btn-confirm"
					aria-label={`Confirm ${cue.recordingTool}`}
					title={`Confirm ${cue.recordingTool}: record it from now and reprice the shots since your last hotbar press`}
					disabled={decidingWeapon}
					onclick={() => onWeaponDecision('confirm')}
				>
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="h-3 w-3" aria-hidden="true">
						<path fill-rule="evenodd" d="M12.416 3.376a.75.75 0 0 1 .208 1.04l-5 7.5a.75.75 0 0 1-1.154.114l-3-3a.75.75 0 0 1 1.06-1.06l2.353 2.353 4.493-6.74a.75.75 0 0 1 1.04-.207Z" clip-rule="evenodd" />
					</svg>
				</button>
				<button
					type="button"
					class="cue-btn"
					aria-label={`Keep ${cue.hotbarTool}`}
					title={`Keep ${cue.hotbarTool}: price these shots to it`}
					disabled={decidingWeapon}
					onclick={() => onWeaponDecision('keep')}
				>
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="h-3 w-3" aria-hidden="true">
						<path d="M5.28 4.22a.75.75 0 0 0-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 1 0 1.06 1.06L8 9.06l2.72 2.72a.75.75 0 1 0 1.06-1.06L9.06 8l2.72-2.72a.75.75 0 0 0-1.06-1.06L8 6.94 5.28 4.22Z" />
					</svg>
				</button>
			</div>
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

	<!-- Armour cost: recorded when repairing or scanning, and spread back over
		 the sessions it covers, so it needs no running session. -->
	<div
		class="flex items-center gap-2 shrink-0 border-l border-white/10 pl-3"
		data-guide-anchor="overlay-armour-section"
	>
		<span class="text-white/40 shrink-0">{@html ICON_ARMOUR}</span>
		<button
			class="px-2 py-0.5 rounded-[4px] border text-[9px] font-medium transition-all cursor-pointer
				{armourCostOpen
					? 'bg-accent/20 border-accent/40 text-accent'
					: 'bg-white/5 border-white/10 text-white/60 hover:bg-white/10 hover:text-white/90'}"
			aria-haspopup="dialog"
			aria-expanded={armourCostOpen}
			onclick={onArmourCostToggle}
			title="Record an armour repair or a limited set's reading"
			data-guide-anchor="overlay-armour-cost-btn"
		>
			Cost
		</button>
	</div>

	<!-- Doses: what a consumable has in force, each counting down to its
		 stored end, with the one correction a misclicked key needs. -->
	{#if doses}
		<OverlayDoses model={doses} menuOpen={dosesMenuOpen} onStartTrigger={onDosesTrigger} />
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
						<span
							class="text-sm font-semibold tabular-nums leading-none {valueColor}"
							title={r.incomplete}
						>{r.value}{#if r.incomplete}<span
									class="ml-px align-super text-[9px] font-normal text-amber-300/90"
									aria-hidden="true">*</span
								><span class="sr-only">, {r.incomplete}</span>{/if}</span>
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

	.cue-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.6);
		transition: all 150ms ease-out;
	}
	.cue-btn:hover:not(:disabled),
	.cue-btn:focus-visible {
		background: rgba(255, 255, 255, 0.12);
		color: rgba(255, 255, 255, 0.95);
		border-color: rgba(255, 255, 255, 0.3);
		outline: none;
	}
	.cue-btn-confirm:hover:not(:disabled),
	.cue-btn-confirm:focus-visible {
		background: rgba(52, 211, 153, 0.18);
		border-color: rgba(52, 211, 153, 0.45);
		color: rgb(110, 231, 183);
	}
	.cue-btn:disabled {
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
</style>
