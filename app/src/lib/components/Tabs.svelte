<script lang="ts">
	type Tab = {
		id: string;
		label: string;
		/** Something new arrived in this tab since it was last open. */
		attention?: boolean;
	};

	let {
		tabs,
		active,
		onchange,
		class: className = ''
	}: {
		tabs: Tab[];
		active: string;
		onchange: (id: string) => void;
		class?: string;
	} = $props();
</script>

<div class="relative flex gap-1 border-b border-border/70 {className}" role="tablist">
	{#each tabs as tab}
		<button
			role="tab"
			data-tab-id={tab.id}
			aria-selected={active === tab.id}
			class="relative px-3.5 py-2 text-sm font-medium cursor-pointer
				transition-colors duration-[var(--duration-base)] ease-[var(--ease-out)]
				focus-visible:outline-none focus-visible:text-text
				{active === tab.id
				? 'text-accent'
				: 'text-text-secondary hover:text-text'}"
			onclick={() => onchange(tab.id)}
		>
			{tab.label}
			{#if tab.attention && active !== tab.id}
				<span class="relative ml-1 inline-flex h-2 w-2 -translate-y-1.5" data-testid="tab-attention">
					<span aria-hidden="true" class="absolute inset-0 rounded-full bg-warning opacity-75 animate-ping"></span>
					<span aria-hidden="true" class="relative h-2 w-2 rounded-full bg-warning
						[box-shadow:0_0_8px_color-mix(in_oklab,var(--color-warning)_70%,transparent)]"></span>
					<span class="sr-only">, new</span>
				</span>
			{/if}
			{#if active === tab.id}
				<span
					aria-hidden="true"
					class="pointer-events-none absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-accent
						[box-shadow:0_0_10px_color-mix(in_oklab,var(--color-accent)_70%,transparent)]"
				></span>
			{/if}
		</button>
	{/each}
</div>
