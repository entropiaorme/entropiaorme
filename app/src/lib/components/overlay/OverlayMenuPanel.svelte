<script lang="ts">
	import { formatDuration } from '$lib/features/consumables/doses';
	import { formatTimeUntil } from '$lib/features/quests/cooldown';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import type { ActivityOption } from '$lib/api';
	import type { OverlayMenuSelection, OverlayMenuState } from '$lib/windows/overlayMenu';

	let {
		menuState,
		onSelect,
		onActivitySelect
	}: {
		menuState: OverlayMenuState;
		/** A pick that closes the menu (every kind but an Activities action). */
		onSelect: (selection: OverlayMenuSelection) => void;
		/** An Activities action, which keeps the control open (the overlay
		 * re-shows it with the refreshed rows), so declaring one thing after
		 * another is not a close-and-reopen. */
		onActivitySelect: (selection: OverlayMenuSelection) => void;
	} = $props();

	// The gate countdowns tick while the control is open, so a row that
	// becomes available says so without the user reopening it.
	let now = $state(Date.now());
	$effect(() =>
		useVisiblePoll(() => {
			now = Date.now();
		}, { intervalMs: 1000 })
	);

	// A view of the model's buffer, not a second copy of it: each
	// presentation restores what was typed, so a refused declaration
	// hands the name back rather than eating it.
	let segmentDraft = $state('');
	$effect(() => {
		if (menuState.kind !== 'activities') return;
		segmentDraft = menuState.segmentDraft;
	});

	/** What a row says about itself on its right: the standing state, or
	 * why it cannot be declared (with the gate counted down where there
	 * is one). An available row says nothing: the name is the point, and
	 * how a quest completes is the record's business, not the player's
	 * at the moment of choosing. */
	function rowBadge(option: ActivityOption): string | null {
		if (option.active && option.handInWaiting) return 'Waiting';
		if (option.active) return 'Recording';
		if (option.available) return null;
		const left = option.availableFrom === null ? null : formatTimeUntil(option.availableFrom, now);
		if (!option.unavailableReason) return left;
		return left ? `${option.unavailableReason} · ${left}` : option.unavailableReason;
	}

	function rowTitle(option: ActivityOption, idle: boolean): string {
		if (option.active) return 'Recording this; tap to stop recording it';
		if (!option.available) return option.unavailableReason ?? '';
		if (idle) return 'Start tracking to record this';
		return 'What you do next counts toward this';
	}

	function declareTyped() {
		const label = segmentDraft.trim();
		if (!label) return;
		// Cleared by the re-presentation when the write lands, and
		// restored by it when the write is refused.
		onActivitySelect({ kind: 'activities', action: 'declare', label });
	}
</script>

<div class="menu-panel">
	{#if menuState.kind === 'activities'}
		{#if menuState.options.length === 0 && !menuState.adHocSegments}
			<div class="menu-empty">Nothing to declare</div>
		{:else}
			{@const anyActive = menuState.options.some((option) => option.active)}
			{@const idle = menuState.idle}
			{#each menuState.options as option (option.key)}
				{@const badge = rowBadge(option)}
				<div class="menu-row">
					<button
						type="button"
						class="menu-option {option.active ? 'menu-option-active' : ''}"
						disabled={idle || (!option.available && !option.active)}
						title={rowTitle(option, idle)}
						onclick={() => onActivitySelect({ kind: 'activities', action: 'toggle', key: option.key })}
					>
						<span class="menu-option-name">{option.name}</span>
						{#if badge}
							<span
								class="menu-option-badge {option.active ? '' : 'menu-option-badge-muted'}"
							>{badge}</span>
						{/if}
					</button>
					<!-- Co-activation is deliberate, never ambient: the affordance
						 appears only once something is standing for the row to join. -->
					{#if !idle && !option.active && option.available && anyActive}
						<button
							type="button"
							class="menu-join-btn"
							aria-label={`Also record ${option.name}`}
							title="Also record this, alongside what is already running"
							onclick={() =>
								onActivitySelect({ kind: 'activities', action: 'coActivate', key: option.key })}
						>+</button>
					{/if}
					{#if !idle && option.active && option.manualHandIn}
						<button
							type="button"
							class="menu-hand-in-btn"
							aria-label={`Hand in ${option.name}`}
							title="Review the quest reward clump"
							onclick={() =>
								onActivitySelect({ kind: 'activities', action: 'handIn', key: option.key })}
						>Hand in quest</button>
					{/if}
				</div>
			{/each}
			{#if menuState.adHocSegments}
				<!-- Naming as you play: this session declared it has structure
					 it invents on the spot, and a name typed here becomes one of
					 its rows next time. -->
				<div class="menu-row menu-entry">
					<input
						class="menu-input"
						bind:value={segmentDraft}
						placeholder={idle ? 'Start tracking to name one' : 'Name what you are doing...'}
						aria-label="Name this activity"
						disabled={idle}
						onkeydown={(event) => {
							if (event.key !== 'Enter') return;
							event.preventDefault();
							declareTyped();
						}}
					/>
					<button
						type="button"
						class="menu-join-btn"
						aria-label="Record under this name"
						title="Record what you do next under this name"
						disabled={idle || !segmentDraft.trim()}
						onclick={declareTyped}
					>&rarr;</button>
				</div>
			{/if}
		{/if}
	{:else if menuState.kind === 'questHandIn'}
		<!-- Rendered by the dedicated hand-in panel in the popup route. -->
	{:else if menuState.kind === 'definition'}
		{#if menuState.definitions.length === 0}
			<div class="menu-empty">Sessions unavailable; open the dashboard</div>
		{:else}
			{#each menuState.definitions as definition (definition.id)}
				<button
					type="button"
					class="menu-option {definition.selected ? 'menu-option-active' : ''}"
					title={definition.selected
						? 'Already selected for the next session'
						: 'Record the next session under this one'}
					onclick={() =>
						onSelect({
							kind: 'definition',
							definitionId: definition.id,
							selected: definition.selected
						})}
				>
					<span class="menu-option-name">{definition.name}</span>
					{#if definition.selected}
						<span class="menu-option-badge">Selected</span>
					{/if}
				</button>
			{/each}
		{/if}
	{:else if menuState.kind === 'consumables'}
		{#if menuState.options.length === 0}
			<div class="menu-empty">No consumables in Equipment</div>
		{:else}
			{#each menuState.options as option (option.equipmentId)}
				{@const running = menuState.runningIds.includes(option.equipmentId)}
				<button
					type="button"
					class="menu-option {running ? 'menu-option-active' : ''}"
					title={running
						? `Take another dose of ${option.name}; the running one ends now`
						: `Take a dose of ${option.name}`}
					onclick={() => onSelect({ kind: 'consumables', equipmentId: option.equipmentId })}
				>
					<span class="menu-option-name">{option.name}</span>
					<span class="menu-option-badge {running ? '' : 'menu-option-badge-muted'}">
						{running ? 'Running' : formatDuration(option.durationSeconds)}{option.hotbarSlot
							? ` · key ${option.hotbarSlot}`
							: ''}
					</span>
				</button>
			{/each}
		{/if}
	{:else if menuState.loading}
		<div class="menu-empty">Searching...</div>
	{:else if menuState.error}
		<div class="menu-empty">{menuState.error}</div>
	{:else if menuState.kind === 'mob'}
		{#if menuState.mobSuggestions.length === 0}
			<div class="menu-empty">No matches</div>
		{:else}
			{#each menuState.mobSuggestions as option}
				<button
					type="button"
					class="menu-option"
					onclick={() => onSelect({ kind: 'mob', species: option.species, maturity: option.maturity })}
				>
					<span class="menu-option-name">{option.display}</span>
				</button>
			{/each}
		{/if}
	{/if}
</div>

<style>
	.menu-panel {
		display: flex;
		flex-direction: column;
		width: 100%;
		max-height: 220px;
		overflow-y: auto;
		padding: 6px;
		border-radius: 8px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: rgba(11, 15, 25, 0.96);
		backdrop-filter: blur(16px) saturate(150%);
		box-shadow:
			0 14px 30px rgba(0, 0, 0, 0.48),
			0 0 0 1px rgba(255, 255, 255, 0.03);
	}

	.menu-option {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		width: 100%;
		padding: 7px 8px;
		border: none;
		border-radius: 6px;
		background: transparent;
		color: rgba(255, 255, 255, 0.82);
		font-size: 12px;
		text-align: left;
		cursor: pointer;
		transition:
			background-color 140ms ease,
			color 140ms ease;
	}

	.menu-option:hover,
	.menu-option:focus-visible {
		background: rgba(255, 255, 255, 0.06);
		color: rgba(255, 255, 255, 0.94);
		outline: none;
	}

	.menu-option-active {
		background: rgba(56, 189, 248, 0.14);
		color: rgba(186, 230, 253, 0.98);
	}

	.menu-option:disabled {
		opacity: 0.45;
		cursor: default;
	}
	.menu-option:disabled:hover {
		background: transparent;
		color: rgba(255, 255, 255, 0.82);
	}

	.menu-hand-in-btn {
		flex-shrink: 0;
		padding: 5px 7px;
		border: 0;
		border-radius: 5px;
		background: rgba(56, 189, 248, 0.14);
		color: rgba(186, 230, 253, 0.98);
		font-size: 10px;
		font-weight: 650;
		cursor: pointer;
	}

	.menu-hand-in-btn:hover,
	.menu-hand-in-btn:focus-visible {
		background: rgba(56, 189, 248, 0.24);
		outline: none;
	}

	.menu-option-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.menu-option-badge {
		flex-shrink: 0;
		padding: 2px 6px;
		border-radius: 999px;
		background: rgba(56, 189, 248, 0.16);
		color: rgba(186, 230, 253, 0.96);
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.02em;
	}

	.menu-option-badge-muted {
		background: rgba(255, 255, 255, 0.07);
		color: rgba(255, 255, 255, 0.45);
	}

	.menu-empty {
		padding: 8px 10px;
		color: rgba(255, 255, 255, 0.45);
		font-size: 11px;
	}

	.menu-row {
		display: flex;
		align-items: center;
		gap: 4px;
	}
	.menu-row .menu-option {
		flex: 1;
		min-width: 0;
	}

	.menu-join-btn {
		flex-shrink: 0;
		width: 22px;
		height: 22px;
		border-radius: 5px;
		border: 1px solid rgba(255, 255, 255, 0.15);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.5);
		font-size: 12px;
		line-height: 1;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: all 140ms ease;
	}
	.menu-join-btn:hover,
	.menu-join-btn:focus-visible {
		background: rgba(56, 189, 248, 0.16);
		border-color: rgba(56, 189, 248, 0.4);
		color: rgba(186, 230, 253, 0.96);
		outline: none;
	}

	.menu-join-btn:disabled {
		opacity: 0.3;
		cursor: default;
	}
	.menu-join-btn:disabled:hover {
		background: rgba(255, 255, 255, 0.05);
		border-color: rgba(255, 255, 255, 0.15);
		color: rgba(255, 255, 255, 0.5);
	}

	/* The free-text row sits under the offerings, separated by a hairline
	   so it reads as a different gesture from picking one. */
	.menu-entry {
		margin-top: 4px;
		padding-top: 6px;
		border-top: 1px solid rgba(255, 255, 255, 0.08);
	}

	.menu-input {
		flex: 1;
		min-width: 0;
		padding: 5px 8px;
		border-radius: 6px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: rgba(255, 255, 255, 0.04);
		color: rgba(255, 255, 255, 0.9);
		font-size: 12px;
		outline: none;
		transition:
			border-color 140ms ease,
			background-color 140ms ease;
	}
	.menu-input::placeholder {
		color: rgba(255, 255, 255, 0.3);
	}
	.menu-input:focus {
		border-color: rgba(56, 189, 248, 0.45);
		background: rgba(255, 255, 255, 0.07);
	}
</style>
