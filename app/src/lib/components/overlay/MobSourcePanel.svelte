<script lang="ts">
	// Where the session's mob comes from, as a sub-window of the overlay's
	// mob line: what the screen reads now, what recent kills suggest, the
	// typed backstop, and a field to type a mob, to pin it or keep it as the
	// backstop. What it shows is the host's; the typing is its own.
	import { ApiError, type ManualMobSuggestion } from '$lib/api';
	import { createTypeahead } from '$lib/view/typeahead.svelte';
	import { NO_DATA } from '$lib/utils/format';
	import type { OverlayMenuSelection, OverlayMobSourceState } from '$lib/windows/overlayMenu';
	import SubWindowPanel from './SubWindowPanel.svelte';

	let {
		panelState,
		search,
		onSelect,
		onDone,
	}: {
		panelState: OverlayMobSourceState;
		/** The catalogue mobs matching what is typed. */
		search: (query: string) => Promise<ManualMobSuggestion[]>;
		/** What the player chose; the panel stays for the next thing. */
		onSelect: (selection: OverlayMenuSelection) => void;
		/** The player is done here: close. */
		onDone: () => void;
	} = $props();

	/** The mob picked from the suggestions, waiting to be told what for. */
	let picked = $state<ManualMobSuggestion | null>(null);
	let query = $state('');

	const typeahead = createTypeahead<ManualMobSuggestion>({
		search: async (text) => {
			try {
				return await search(text);
			} catch (e) {
				throw new Error(e instanceof ApiError ? e.message : 'Mob lookup failed');
			}
		},
		debounceMs: 120,
		minLength: 1,
	});
	$effect(() => () => typeahead.destroy());

	const typed = $derived(query.trim());
	const searching = $derived(typed !== '' && picked === null);

	function type(value: string) {
		query = value;
		picked = null;
		typeahead.query = value;
	}

	function pick(option: ManualMobSuggestion) {
		picked = option;
		query = option.display;
		typeahead.cancel();
	}

	function use(action: 'pin' | 'backstop') {
		const mob = picked;
		if (!mob) return;
		onSelect({ kind: 'mobSource', action, species: mob.species, maturity: mob.maturity });
		picked = null;
		query = '';
		// Pinned, the mob is settled; a backstop set leaves the rest to read.
		if (action === 'pin') onDone();
	}

	/** Enter takes the top match: only a catalogue mob can be entered. */
	function keydown(event: KeyboardEvent) {
		if (event.key !== 'Enter' || picked) return;
		const top = typeahead.results[0];
		if (!top) return;
		event.preventDefault();
		pick(top);
	}
</script>

<SubWindowPanel heading="Mob" label="Where the mob comes from" testId="mob-source-panel">
	{#each panelState.lines as line (line.kind)}
		<div class="panel-section" data-testid="mob-line" data-kind={line.kind}>
			<div class="flex items-baseline justify-between gap-3">
				<span class="panel-key">{line.label}</span>
				{#if line.kind === 'backstop' && line.creature}
					<button
						type="button"
						class="panel-link"
						disabled={panelState.saving}
						onclick={() => onSelect({ kind: 'mobSource', action: 'clearBackstop' })}
					>
						Clear
					</button>
				{/if}
			</div>
			<div class="panel-value {line.creature ? '' : 'panel-faint'}">
				{line.creature?.name ?? NO_DATA}
			</div>
			{#if line.note}<p class="panel-note">{line.note}</p>{/if}
		</div>
	{/each}
	{#if panelState.canType}
		<div class="panel-section">
			<input
				class="mob-input"
				value={query}
				placeholder="Type a mob..."
				aria-label="Mob"
				disabled={panelState.saving}
				oninput={(event) => type(event.currentTarget.value)}
				onkeydown={keydown}
			/>
			{#if picked}
				<div class="mt-2 flex items-center gap-2" data-testid="mob-draft">
					<span class="panel-value min-w-0 flex-1">{picked.display}</span>
					<button type="button" class="panel-chip" disabled={panelState.saving} onclick={() => use('pin')}>
						Pin
					</button>
					{#if !panelState.pinned}
						<button
							type="button"
							class="panel-chip"
							disabled={panelState.saving}
							onclick={() => use('backstop')}
						>
							Use as backstop
						</button>
					{/if}
				</div>
			{:else if searching}
				<ul class="suggestions" role="listbox" aria-label="Matching mobs" data-testid="mob-suggestions">
					{#if typeahead.error}
						<li class="panel-note">{typeahead.error}</li>
					{:else if typeahead.results.length > 0}
						{#each typeahead.results as option (option.display)}
							<li role="option" aria-selected="false">
								<button type="button" class="suggestion" onclick={() => pick(option)}>
									{option.display}
								</button>
							</li>
						{/each}
					{:else if typeahead.loading || typeahead.settledQuery !== typed}
						<li class="panel-note">Searching...</li>
					{:else}
						<li class="panel-note">No matches</li>
					{/if}
				</ul>
			{/if}
		</div>
	{/if}
</SubWindowPanel>

<style>
	.mob-input {
		width: 100%;
		padding: 2px 0 3px;
		border: none;
		border-bottom: 1px solid rgba(255, 255, 255, 0.12);
		background: transparent;
		color: rgba(255, 255, 255, 0.95);
		font-size: 13px;
		outline: none;
	}
	.mob-input:focus {
		border-bottom-color: #38bdf8;
	}
	.mob-input::placeholder {
		color: rgba(255, 255, 255, 0.28);
	}
	.suggestions {
		max-height: 9.5rem;
		margin: 6px 0 0;
		padding: 0;
		overflow-y: auto;
		list-style: none;
	}
	.suggestion {
		display: block;
		width: 100%;
		padding: 4px 6px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: rgba(255, 255, 255, 0.9);
		font-size: 12px;
		text-align: left;
		cursor: pointer;
	}
	.suggestion:hover,
	.suggestion:focus-visible {
		background: rgba(255, 255, 255, 0.08);
		outline: none;
	}
</style>
