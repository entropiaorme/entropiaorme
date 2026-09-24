<script lang="ts">
	import type { TrackingLive } from '$lib/api';

	type TrifectaAttribution = NonNullable<TrackingLive['trifectaAttribution']>;

	let {
		trifecta,
		tone = 'active',
		menuOpen = false,
		disabled = false,
		ontrigger
	}: {
		trifecta: TrifectaAttribution | null | undefined;
		tone?: 'active' | 'idle';
		menuOpen?: boolean;
		disabled?: boolean;
		ontrigger?: (anchor: HTMLButtonElement) => Promise<void> | void;
	} = $props();

	let trigger: HTMLButtonElement | null = $state(null);

	const label = $derived.by(() => {
		const presetName = trifecta?.presetName?.trim();
		if (presetName) return presetName;

		const fallback = [
			trifecta?.smallWeapon,
			trifecta?.bigWeapon,
			trifecta?.healTool
		].filter((value): value is string => Boolean(value?.trim()));

		return fallback.length > 0 ? fallback.join(' / ') : '—';
	});

	async function handleTrigger() {
		if (!trigger || !trifecta || disabled) return;
		await ontrigger?.(trigger);
	}
</script>

<div class="trifecta-shell">
	<button
		bind:this={trigger}
		type="button"
		class="trifecta-trigger {tone === 'idle' ? 'trifecta-trigger-idle' : ''}"
		aria-haspopup="menu"
		aria-expanded={menuOpen}
		title={label}
		disabled={disabled || !trifecta}
		onclick={handleTrigger}
	>
		<span class="trifecta-label">{label}</span>
		<span class="trifecta-chevron {menuOpen ? 'trifecta-chevron-open' : ''}">▾</span>
	</button>
</div>

<style>
	.trifecta-shell {
		position: relative;
		margin-top: 2px;
	}

	.trifecta-trigger {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		width: 100%;
		padding: 4px 7px;
		border-radius: 6px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: linear-gradient(180deg, rgba(255, 255, 255, 0.07), rgba(255, 255, 255, 0.03));
		color: rgba(255, 255, 255, 0.78);
		font-size: 12px;
		text-align: left;
		transition:
			border-color 140ms ease,
			background-color 140ms ease,
			color 140ms ease,
			transform 140ms ease;
	}

	.trifecta-trigger:hover:not(:disabled),
	.trifecta-trigger:focus-visible {
		border-color: rgba(125, 211, 252, 0.38);
		background: linear-gradient(180deg, rgba(56, 189, 248, 0.12), rgba(255, 255, 255, 0.05));
		color: rgba(255, 255, 255, 0.92);
		outline: none;
	}

	.trifecta-trigger:disabled {
		cursor: default;
		opacity: 0.8;
	}

	.trifecta-trigger-idle {
		color: rgba(255, 255, 255, 0.6);
	}

	.trifecta-label {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.trifecta-chevron {
		flex-shrink: 0;
		font-size: 10px;
		color: rgba(255, 255, 255, 0.45);
		transition: transform 140ms ease, color 140ms ease;
	}

	.trifecta-chevron-open {
		transform: rotate(180deg);
		color: rgba(125, 211, 252, 0.8);
	}
</style>
