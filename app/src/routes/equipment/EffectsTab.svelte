<script lang="ts">
	import { updateSettings } from '$lib/api';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import Toggle from '$lib/components/Toggle.svelte';
	import { describeReloadLimit } from '$lib/features/equipment/attackRate';
	import type {
		AppSettings,
		PassiveEffectSourceView,
		ReloadSpeedInEffect,
	} from '$lib/types/settings';

	let {
		sources: initialSources,
		reloadSpeed = null,
		onchange,
	}: {
		sources: PassiveEffectSourceView[];
		/** The reload speed the saved sources put in force. */
		reloadSpeed?: ReloadSpeedInEffect | null;
		onchange?: (settings: AppSettings) => void;
	} = $props();

	let sources = $state<PassiveEffectSourceView[]>([]);
	let saving = $state(false);
	let dirty = $state(false);
	let error = $state<string | null>(null);

	// Describes the saved sources, so it stands aside while edits are unsaved.
	const limitNote = $derived(dirty ? null : describeReloadLimit(reloadSpeed));

	$effect(() => {
		sources = initialSources.map((source) => ({
			...source,
			effects: source.effects.map((effect) => ({ ...effect })),
		}));
		dirty = false;
	});

	function updateSource(index: number, patch: Partial<PassiveEffectSourceView>) {
		sources[index] = { ...sources[index], ...patch };
		sources = [...sources];
		dirty = true;
	}

	function updateMagnitude(index: number, value: string) {
		const magnitude = Number(value);
		if (!Number.isFinite(magnitude)) return;
		updateSource(index, {
			effects: [{ kind: 'reload_speed', magnitudePercent: magnitude }],
		});
	}

	function addSource() {
		const id = globalThis.crypto?.randomUUID?.() ?? `effect-${Date.now()}`;
		sources = [
			...sources,
			{
				id,
				name: '',
				enabled: true,
				effects: [{ kind: 'reload_speed', magnitudePercent: 0 }],
			},
		];
		dirty = true;
	}

	function removeSource(index: number) {
		sources = sources.filter((_, candidate) => candidate !== index);
		dirty = true;
	}

	const valid = $derived(
		sources.every(
			(source) =>
				source.name.trim().length > 0 &&
				Number.isFinite(source.effects[0]?.magnitudePercent),
		) &&
			sources
				.filter((source) => source.enabled)
				.reduce((total, source) => total + (source.effects[0]?.magnitudePercent ?? 0), 0) > -100,
	);

	async function save() {
		if (!dirty || !valid || saving) return;
		saving = true;
		error = null;
		try {
			const updated = await updateSettings({
				passive_effect_sources: sources.map((source) => ({
					id: source.id,
					name: source.name.trim(),
					enabled: source.enabled,
					effects: source.effects.map((effect) => ({
						kind: effect.kind,
						magnitude_percent: effect.magnitudePercent,
					})),
				})),
			});
			sources = updated.passiveEffectSources.map((source) => ({
				...source,
				effects: source.effects.map((effect) => ({ ...effect })),
			}));
			dirty = false;
			onchange?.(updated);
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Passive effects could not be saved';
		} finally {
			saving = false;
		}
	}
</script>

<div class="space-y-5">
	<div class="flex items-start justify-between gap-6">
		<div>
			<h2 class="text-sm font-medium text-text">Passive effects</h2>
			<p class="mt-1 max-w-xl text-sm leading-6 text-text-secondary">
				Declare persistent effects from equipped clothing or accessories. Reload speed shortens
				the interval used to recognise paid healing-tool activations, and past the server's
				100 attacks a minute it makes each weapon attack land harder and cost more.
			</p>
		</div>
		<Button size="sm" variant="secondary" onclick={addSource}>Add effect</Button>
	</div>

	{#if error}
		<p class="border-l-2 border-error/70 pl-3 text-sm text-error">{error}</p>
	{/if}

	{#if sources.length === 0}
		<p class="py-5 text-sm text-text-tertiary">
			No passive effects declared. Weapons and healing tools use their catalogue rates.
		</p>
	{:else}
		<div class="border-t border-border/70">
			{#each sources as source, index (source.id)}
				<div class="grid grid-cols-[auto_minmax(12rem,1fr)_10rem_auto] items-end gap-4 border-b border-border/70 py-3">
					<Toggle
						checked={source.enabled}
						label={`Enable ${source.name || 'passive effect'}`}
						onchange={(enabled) => updateSource(index, { enabled })}
					/>
					<label class="block">
						<span class="mb-1 block text-xs text-text-secondary">Source</span>
						<Input
							value={source.name}
							placeholder="Ares Ring, Perfected"
							oninput={(event) => updateSource(index, { name: event.currentTarget.value })}
						/>
					</label>
					<label class="block">
						<span class="mb-1 block text-xs text-text-secondary">Reload speed</span>
						<div class="flex items-center gap-2">
							<Input
								class="w-24"
								type="number"
								step="0.1"
								value={String(source.effects[0]?.magnitudePercent ?? 0)}
								oninput={(event) => updateMagnitude(index, event.currentTarget.value)}
							/>
							<span class="text-sm text-text-tertiary">%</span>
						</div>
					</label>
					<Button variant="ghost" size="sm" onclick={() => removeSource(index)}>Remove</Button>
				</div>
			{/each}
		</div>
	{/if}

	{#if limitNote}
		<p class="text-xs leading-5 text-text-secondary" data-testid="reload-limit-note">{limitNote}</p>
	{/if}

	<div class="flex items-center justify-end gap-3">
		{#if dirty && !valid}<span class="text-xs text-error">Name every source and keep combined reload speed above -100%.</span>{/if}
		<Button size="sm" disabled={!dirty || !valid} loading={saving} onclick={save}>Save effects</Button>
	</div>
</div>
