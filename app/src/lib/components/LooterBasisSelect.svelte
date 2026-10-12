<script lang="ts">
	import Menu from './Menu.svelte';
	import {
		LOOTER_BASES,
		looterBasis,
		looterBasisLevel,
		looterBasisName,
		setLooterBasis,
		type LooterBasis,
		type LooterLevels,
	} from '$lib/looterBasis.svelte';

	// The looter basis an expected figure is read at, named inline where the
	// figure discloses its model: the trigger reads as part of that caption
	// ("Animal Looter 45.0") and opens the four bases with their levels.
	// It follows the app-wide basis unless a caller owns the choice.
	let {
		levels,
		basis: ownBasis,
		onchange = (next: LooterBasis) => void setLooterBasis(next),
		class: className = '',
	}: {
		levels: LooterLevels;
		basis?: LooterBasis;
		onchange?: (basis: LooterBasis) => void;
		class?: string;
	} = $props();

	const basis = $derived(ownBasis ?? looterBasis.current);
	const level = (chosen: LooterBasis) => looterBasisLevel(levels, chosen).toFixed(1);
	const current = $derived(`${looterBasisName(basis)} ${level(basis)}`);
</script>

<Menu overlay align="left" panelClass="w-56 p-1" class="inline-block align-baseline {className}">
	{#snippet trigger({ open, toggle, keydown })}
		<button
			type="button"
			class="-mx-1 inline-flex cursor-pointer items-baseline gap-1 rounded px-1 tabular-nums
				text-text-secondary transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover
				hover:text-text focus:outline-none focus-visible:[box-shadow:var(--shadow-glow)]"
			aria-haspopup="menu"
			aria-expanded={open}
			aria-label="Looter basis: {current}"
			data-testid="looter-basis-trigger"
			onclick={toggle}
			onkeydown={keydown}
		>
			{current}
			<svg
				class="h-2.5 w-2.5 self-center transition-transform {open ? 'rotate-180' : ''}"
				viewBox="0 0 20 20"
				fill="currentColor"
				aria-hidden="true"
			>
				<path
					fill-rule="evenodd"
					d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
					clip-rule="evenodd"
				/>
			</svg>
		</button>
	{/snippet}

	{#snippet children({ close })}
		<p class="eyebrow px-2 pb-1 pt-1.5">Read at</p>
		{#each LOOTER_BASES as option (option)}
			{@const chosen = option === basis}
			<button
				type="button"
				role="menuitemradio"
				aria-checked={chosen}
				class="flex w-full cursor-pointer items-center justify-between gap-3 rounded px-2 py-1.5 text-left text-xs
					transition-colors duration-[var(--duration-fast)]
					{chosen ? 'bg-accent/10 text-accent' : 'text-text-secondary hover:bg-surface-hover hover:text-text'}"
				onclick={() => {
					close();
					if (!chosen) onchange(option);
				}}
			>
				<span>{option === 'three_looter_mean' ? 'Mean of the three' : looterBasisName(option)}</span>
				<span class="tabular-nums {chosen ? '' : 'text-text-tertiary'}">{level(option)}</span>
			</button>
		{/each}
	{/snippet}
</Menu>
