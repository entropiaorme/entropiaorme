<script lang="ts">
	import { setNanocubeMark } from '$lib/api';

	/**
	 * Mark an item to be valued at the Nanocube markup in the live session
	 * figures, or lift the mark, in one click. For an item whose market
	 * reading the player does not believe: the mark stands across sessions
	 * and imports until lifted (or until the player enters their own value),
	 * and only the live figures read it.
	 */
	let {
		itemName,
		marked,
		onChanged,
		onError,
	}: {
		itemName: string;
		marked: boolean;
		onChanged?: () => void;
		onError?: (message: string) => void;
	} = $props();

	let busy = $state(false);

	async function toggle() {
		busy = true;
		try {
			await setNanocubeMark(itemName, !marked);
			onChanged?.();
		} catch {
			onError?.(`Could not ${marked ? 'lift the mark on' : 'mark'} ${itemName}`);
		} finally {
			busy = false;
		}
	}

	const label = $derived(
		marked
			? `Lift the Nanocube mark on ${itemName}`
			: `Value ${itemName} at Nanocube in the live figures`,
	);
</script>

<button
	type="button"
	class="icon-button-row p-1 {marked ? 'text-warning' : ''}"
	aria-pressed={marked}
	aria-label={label}
	title={marked ? 'Marked as Nanocube in the live figures: click to lift' : 'Value at Nanocube in the live figures'}
	disabled={busy}
	onclick={() => void toggle()}
>
	<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" class="h-3.5 w-3.5" aria-hidden="true">
		<circle cx="10" cy="10" r="6.25" />
		<path d="M5.6 14.4 14.4 5.6" stroke-linecap="round" />
	</svg>
</button>
