<script lang="ts">
	import type { Component } from 'svelte';
	import { theme } from '$lib/theme.svelte';

	export type NavItem = {
		id: string;
		label: string;
		icon: Component; // icon component rendering the glyph's SVG
		indicator?: 'unread';
	};

	let {
		items,
		active,
		onnavigate,
		footerItems = [],
		settingsItem,
		disabled = false,
		class: className = ''
	}: {
		items: NavItem[];
		active: string;
		onnavigate: (id: string) => void;
		footerItems?: NavItem[];
		settingsItem?: NavItem;
		/** Inert and dimmed: nothing behind the nav can load (a failed start). */
		disabled?: boolean;
		class?: string;
	} = $props();

	let expanded = $state(false);
</script>

<!-- Kept: hover/focus expansion is decorative; every nav item is a native button, reachable collapsed or expanded. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<nav
	class="relative flex flex-col h-full overflow-hidden
		bg-surface/40 backdrop-blur-[2px]
		transition-[width] duration-[var(--duration-slow)] ease-[var(--ease-out)]
		{expanded ? 'w-[200px]' : 'w-12'}
		{disabled ? 'opacity-40' : ''}
		{className}"
	inert={disabled}
	onmouseenter={() => (expanded = true)}
	onmouseleave={() => (expanded = false)}
	onfocusin={() => (expanded = true)}
	onfocusout={(e) => {
		if (!(e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget))) {
			expanded = false;
		}
	}}
	aria-label="Main navigation"
>
	<!-- Right-edge separator: subtle gradient instead of a hard line -->
	<span
		aria-hidden="true"
		class="pointer-events-none absolute right-0 top-0 bottom-0 w-px
			bg-gradient-to-b from-transparent via-border-bright/70 to-transparent"
	></span>

	<!-- Header: brand title-head; drag region + crossfade between simple icon (collapsed) and wordmark (expanded) -->
	<div
		data-tauri-drag-region
		class="relative h-14 px-3 overflow-hidden flex-shrink-0"
	>
		<img
			src="/icon.png"
			alt=""
			aria-hidden="true"
			class="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 pointer-events-none h-7 w-7 select-none transition-opacity duration-[var(--duration-base)] ease-[var(--ease-out)]
				{expanded ? 'opacity-0' : 'opacity-100'}"
		/>
		<img
			src={theme.current === 'light' ? '/wordmark-on-light.png' : '/wordmark-on-dark.png'}
			alt="EntropiaOrme"
			class="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 pointer-events-none h-10 w-auto select-none transition-opacity duration-[var(--duration-base)] ease-[var(--ease-out)]
				{expanded ? 'opacity-100' : 'opacity-0'}"
		/>
	</div>

	{#snippet navButton(item: NavItem)}
		<button
			class="group relative flex items-center gap-3 h-9 rounded-md px-2.5
				transition-[color,background-color] duration-[var(--duration-base)] ease-[var(--ease-out)]
				cursor-pointer overflow-hidden whitespace-nowrap
				focus-visible:outline-none focus-visible:bg-surface-hover/60
				{active === item.id
				? 'text-accent bg-accent-muted/25'
				: 'text-text-secondary hover:text-text hover:bg-surface-hover/70'}"
			onclick={() => onnavigate(item.id)}
			aria-current={active === item.id ? 'page' : undefined}
			title={!expanded ? item.label : undefined}
		>
			{#if active === item.id}
				<span
					aria-hidden="true"
					class="absolute left-0 top-1.5 bottom-1.5 w-0.5 rounded-r-full bg-accent
						[box-shadow:0_0_8px_color-mix(in_oklab,var(--color-accent)_70%,transparent)]"
				></span>
			{/if}
			<span class="relative flex-shrink-0 w-5 h-5 flex items-center justify-center">
				<item.icon />
				{#if item.indicator === 'unread'}
					<span
						aria-hidden="true"
						class="absolute -top-0.5 -right-0.5 h-2 w-2 rounded-full bg-accent
							[box-shadow:0_0_6px_color-mix(in_oklab,var(--color-accent)_70%,transparent)]"
					></span>
				{/if}
			</span>
			<span
				class="text-sm font-medium tracking-tight transition-opacity duration-[var(--duration-base)]
					{expanded ? 'opacity-100' : 'opacity-0'}"
			>
				{item.label}
			</span>
		</button>
	{/snippet}

	<!-- Nav items -->
	<div class="relative flex flex-col gap-0.5 px-1.5 pb-1.5 flex-1">
		{#each items as item}
			{@render navButton(item)}
		{/each}
	</div>

	<!-- Footer: mascot + footer items (e.g. News & Updates) + settings + session indicator -->
	<div class="relative flex flex-col gap-0.5 p-1.5">
		<div
			class="flex items-center justify-center overflow-hidden pb-2 transition-opacity duration-[var(--duration-slow)] ease-[var(--ease-out)]
				{expanded ? 'opacity-40' : 'opacity-0'}"
			aria-hidden="true"
		>
			<img
				src="/watermark.png"
				alt=""
				class="pointer-events-none w-32 h-auto select-none scale-x-[-1]"
			/>
		</div>
		{#each footerItems as item}
			{@render navButton(item)}
		{/each}
		{#if settingsItem}
			{@render navButton(settingsItem)}
		{/if}
		<div
			class="py-2 flex items-center justify-center"
			title="No active session"
			aria-label="No active session"
		>
			<span class="signal-dot idle"></span>
		</div>
	</div>
</nav>
