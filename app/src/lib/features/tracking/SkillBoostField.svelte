<script lang="ts">
	// The skill boost in force: the labelled percentage of the pill running,
	// declared because it changes how skill gains read. Three declarations:
	// blank claims nothing, a typed 0 declares deliberately unboosted play,
	// and a number declares its magnitude. It stamps each skill gain, so it
	// may be changed while a session runs and never touches gains already
	// stamped.
	import { onMount, untrack } from 'svelte';
	import { getTrackingSnapshot, selectDefinition, setSessionConfig, type TrackingSnapshot } from '$lib/api';
	import Input from '$lib/components/Input.svelte';
	import { createSessionFacets } from './sessionFacets.svelte';

	let snapshot = $state<TrackingSnapshot | null>(null);

	async function refresh() {
		snapshot = await getTrackingSnapshot();
	}

	const facets = createSessionFacets({
		readFacets: () => ({
			name: snapshot?.sessionName ?? null,
			definitionId: snapshot?.sessionDefinitionId ?? null,
			boost: snapshot?.skillBoostPercent ?? null,
		}),
		isSessionActive: () => snapshot?.status === 'active',
		refresh,
		setSessionConfig,
		selectDefinition: (id) => selectDefinition(id),
	});

	onMount(() => {
		void refresh().catch(() => {});
	});

	// The field follows the saved declaration while it is not being edited.
	$effect(() => {
		void snapshot?.skillBoostPercent;
		untrack(() => facets.syncBoostDraft());
	});
</script>

<div class="flex items-start justify-between gap-6" data-testid="skill-boost">
	<div>
		<h2 class="text-sm font-medium text-text">Skill boost</h2>
		<p class="mt-1 max-w-xl text-sm leading-6 text-text-secondary">
			The percentage of the skill pill in force. Leave it blank to claim nothing; enter 0 to
			record deliberately unboosted play. Changing it marks skill gains from that moment on.
		</p>
		{#if facets.facetError}
			<p class="mt-2 border-l-2 border-error/70 pl-3 text-sm text-error">{facets.facetError}</p>
		{/if}
	</div>
	<div class="flex shrink-0 items-center gap-2">
		<Input
			aria-label="Skill boost percent"
			class="w-24"
			inputmode="numeric"
			placeholder="None"
			value={facets.boostDraft}
			disabled={snapshot === null || facets.savingBoost}
			oninput={(event) => {
				facets.boostDraft = event.currentTarget.value;
			}}
			onblur={() => void facets.commitBoost()}
			onkeydown={(event) => {
				if (event.key === 'Enter') {
					event.preventDefault();
					void facets.commitBoost();
				}
			}}
		/>
		<span class="text-sm text-text-tertiary">%</span>
	</div>
</div>
