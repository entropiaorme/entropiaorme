<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';

	let {
		value = $bindable(''),
		placeholder = 'Search…',
		loading = false,
		class: className = '',
		onkeydown,
		...rest
	}: {
		value?: string;
		placeholder?: string;
		loading?: boolean;
		class?: string;
	} & Omit<HTMLInputAttributes, 'value' | 'class' | 'placeholder' | 'type'> = $props();

	let input = $state<HTMLInputElement>();

	function clear() {
		value = '';
		input?.focus();
	}

	// Escape empties a filled box before it reaches anything listening
	// further out (a dialog closes on Escape); an owner that handles the
	// key itself (a picker dismissing its suggestions) prevents the default
	// and keeps its own behaviour.
	function handleKeydown(e: KeyboardEvent & { currentTarget: EventTarget & HTMLInputElement }) {
		onkeydown?.(e);
		if (e.defaultPrevented || e.key !== 'Escape' || value === '') return;
		e.preventDefault();
		e.stopPropagation();
		value = '';
	}
</script>

<div class="relative {className}">
	<svg
		class="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-text-tertiary pointer-events-none"
		xmlns="http://www.w3.org/2000/svg"
		viewBox="0 0 20 20"
		fill="currentColor"
	>
		<path
			fill-rule="evenodd"
			d="M9 3.5a5.5 5.5 0 100 11 5.5 5.5 0 000-11zM2 9a7 7 0 1112.452 4.391l3.328 3.329a.75.75 0 11-1.06 1.06l-3.329-3.328A7 7 0 012 9z"
			clip-rule="evenodd"
		/>
	</svg>

	<input
		bind:this={input}
		type="text"
		bind:value
		{placeholder}
		autocomplete="off"
		spellcheck="false"
		onkeydown={handleKeydown}
		class="w-full h-9 pl-9 pr-8 text-sm bg-surface/70 text-text rounded-md
			border border-border
			placeholder:text-text-tertiary
			transition-[border-color,box-shadow,background-color] duration-[var(--duration-base)] ease-[var(--ease-out)]
			hover:border-border-bright
			focus:outline-none focus:bg-surface focus:border-accent/60 focus:[box-shadow:var(--shadow-glow)]"
		{...rest}
	/>

	{#if loading}
		<svg
			class="absolute right-3 top-1/2 -translate-y-1/2 h-4 w-4 text-text-tertiary animate-spin"
			xmlns="http://www.w3.org/2000/svg"
			fill="none"
			viewBox="0 0 24 24"
		>
			<circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
			<path
				class="opacity-75"
				fill="currentColor"
				d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
			/>
		</svg>
	{:else if value}
		<button
			type="button"
			class="absolute right-2 top-1/2 -translate-y-1/2 h-5 w-5 flex items-center justify-center
				rounded-full text-text-tertiary cursor-pointer
				transition-colors duration-[var(--duration-fast)]
				hover:text-text hover:bg-surface-hover"
			onclick={clear}
			aria-label="Clear search"
		>
			<svg
				xmlns="http://www.w3.org/2000/svg"
				viewBox="0 0 20 20"
				fill="currentColor"
				class="h-3.5 w-3.5"
			>
				<path
					d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
				/>
			</svg>
		</button>
	{/if}
</div>
