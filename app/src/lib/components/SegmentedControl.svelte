<script lang="ts">
	type Option = {
		id: string;
		label: string;
		disabled?: boolean;
		ariaLabel?: string;
		title?: string;
		labelClass?: string;
	};
	type Size = 'sm' | 'md';
	/** `segmented`: options share one track. `keys`: each option is its own
	 * small keycap, and the chosen one lights up in the accent, the way the
	 * game marks the hotbar preset in force. */
	type Variant = 'segmented' | 'keys';

	let {
		options,
		active,
		onchange,
		size = 'sm',
		variant = 'segmented',
		label,
		class: className = ''
	}: {
		options: Option[];
		active: string;
		onchange: (id: string) => void;
		size?: Size;
		variant?: Variant;
		/** Names the group for assistive technology. */
		label?: string;
		class?: string;
	} = $props();

	const containerBase: Record<Variant, string> = {
		segmented: 'inline-flex items-center gap-0.5 rounded-md bg-surface-hover/50 p-0.5',
		keys: 'inline-flex items-center gap-1'
	};
	const optionBase =
		'inline-flex items-center justify-center font-medium rounded transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-40';
	const sizeClasses: Record<Size, string> = {
		sm: 'px-3 py-1 text-xs',
		md: 'px-4 py-1.5 text-sm'
	};
	const keyClasses: Record<Size, string> = {
		sm: 'h-6 w-6 text-xs',
		md: 'h-7 w-7 text-sm'
	};
	const activeCls: Record<Variant, string> = {
		segmented: 'bg-surface text-text shadow-sm',
		keys: 'border-accent/60 bg-accent/15 text-accent [box-shadow:0_0_10px_-2px_color-mix(in_oklab,var(--color-accent)_70%,transparent)]'
	};
	const inactiveCls: Record<Variant, string> = {
		segmented: 'text-text-tertiary hover:text-text',
		keys: 'border-border text-text-tertiary hover:border-border-bright hover:text-text'
	};

	function optionClass(id: string): string {
		const shape =
			variant === 'keys'
				? `${keyClasses[size]} rounded-md border font-semibold tabular-nums focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent/60`
				: sizeClasses[size];
		return `${optionBase} ${shape} ${id === active ? activeCls[variant] : inactiveCls[variant]}`;
	}
</script>

<div class="{containerBase[variant]} {className}" role="group" aria-label={label}>
	{#each options as option (option.id)}
		<button
			type="button"
			aria-label={option.ariaLabel ?? option.label}
			aria-pressed={option.id === active}
			title={option.title}
			class={optionClass(option.id)}
			disabled={option.disabled}
			onclick={() => onchange(option.id)}
		>
			<span class={option.labelClass}>{option.label}</span>
		</button>
	{/each}
</div>
