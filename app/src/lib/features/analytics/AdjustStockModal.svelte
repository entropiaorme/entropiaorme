<script lang="ts">
	import Modal from '$lib/components/Modal.svelte';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import { formatPed, formatQuantity } from '$lib/utils/format';
	import { stockHasNoTt, type TreeCuttingStock } from './treeCuttingModel.svelte';

	let {
		item,
		mode,
		onconfirm,
		oncancel,
	}: {
		item: TreeCuttingStock | null;
		mode: 'remove' | 'shrapnel';
		onconfirm: (itemName: string, quantity: number) => Promise<void>;
		oncancel: () => void;
	} = $props();

	// Stock is entered as PED of TT, which is how it is held and thought of;
	// an item with no TT has none to enter, so it is entered by count.
	let amount = $state(0);
	let saving = $state(false);
	let error = $state<string | null>(null);
	let modalOpen = $state(false);
	let initialisedFor = $state<string | null>(null);
	const byCount = $derived(!!item && stockHasNoTt(item));
	const held = $derived(item ? (byCount ? item.heldQty : item.heldTt) : 0);
	const unitTt = $derived(item && item.heldQty > 0 ? item.heldTt / item.heldQty : 0);
	const atMax = $derived(!!item && Math.abs(amount - held) < 0.005);
	/** The units the entered amount comes to. */
	const units = $derived.by(() => {
		if (!item || !(amount > 0)) return 0;
		if (byCount) return Math.min(item.heldQty, Math.round(amount));
		return unitTt > 0 ? Math.min(item.heldQty, Math.round(amount / unitTt)) : 0;
	});

	$effect(() => {
		if (item && amount > held) amount = held;
	});
	$effect(() => {
		if (item && initialisedFor !== item.itemName) {
			amount = byCount ? item.heldQty : item.heldTt;
			error = null;
			initialisedFor = item.itemName;
			modalOpen = true;
		}
		if (!item) {
			initialisedFor = null;
			modalOpen = false;
		}
	});
	$effect(() => {
		if (item && !modalOpen) oncancel();
	});

	async function submit() {
		if (!item || saving || units <= 0 || amount > held) return;
		saving = true;
		error = null;
		try {
			await onconfirm(item.itemName, units);
			modalOpen = false;
		} catch (e) {
			error = e instanceof Error ? e.message : `Failed to ${mode === 'remove' ? 'remove' : 'convert'} the stock`;
		} finally {
			saving = false;
		}
	}
</script>

{#if item}
	<Modal
		bind:open={modalOpen}
		class="max-w-xs"
		title={mode === 'remove' ? `Remove ${item.itemName}` : 'Convert Shrapnel'}
	>
		<div class="space-y-5">
			<div class="space-y-1.5">
				<Input
					type="number"
					min="0"
					max={held}
					step={byCount ? '1' : '0.01'}
					align="right"
					aria-label={byCount
						? 'How many to remove'
						: mode === 'remove'
							? 'PED to remove'
							: 'PED to convert'}
					bind:value={amount}
				>
					{#snippet suffix()}
						<span class="text-xs font-medium uppercase tracking-wider">
							{byCount ? 'units' : 'PED'}
						</span>
					{/snippet}
				</Input>
				<div class="flex items-baseline justify-between gap-2 text-xs">
					<span class="text-text-tertiary tabular-nums">
						{mode === 'shrapnel'
							? `${formatPed(amount * 1.01)} PED ammo after conversion`
							: byCount
								? `${formatQuantity(item.heldQty)} held, no TT`
								: `${formatPed(item.heldTt)} PED in stock`}
					</span>
					<button
						type="button"
						disabled={atMax}
						onclick={() => (amount = held)}
						class="cursor-pointer font-medium text-accent transition-colors
							duration-[var(--duration-fast)] hover:text-text disabled:cursor-default
							disabled:text-text-tertiary/50 disabled:hover:text-text-tertiary/50"
					>
						All of it
					</button>
				</div>
			</div>

			{#if mode === 'remove'}
				<p class="text-xs leading-relaxed text-text-tertiary">
					This only removes it from current stock. The loot and its historical TT stay recorded.
				</p>
			{/if}
			{#if error}<p class="text-xs text-error">{error}</p>{/if}
			<div class="flex items-center justify-end gap-2">
				<Button variant="ghost" onclick={oncancel} disabled={saving}>Cancel</Button>
				<Button onclick={submit} loading={saving} disabled={units <= 0}>
					{mode === 'remove' ? 'Remove' : 'Convert'}
				</Button>
			</div>
		</div>
	</Modal>
{/if}
