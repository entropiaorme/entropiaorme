<script lang="ts">
	/**
	 * One limited piece: its name, its TT now, and the markup it is billed
	 * at, typed straight into the row and saved when the field is left or
	 * Enter is pressed. Empty bills it at TT. A piece whose TT rose since
	 * its markup was set (a new one of the same name) asks whether the new
	 * one cost the same.
	 */
	import { formatPed, formatSeenAt } from '$lib/utils/format';
	import { MARKUP_REFUSED, markupText, parseMarkup, type Piece, pricingNote } from './armour';

	let {
		piece,
		saving,
		busy,
		error = null,
		onsave,
		onundeclare,
	}: {
		piece: Piece;
		/** This piece is being saved. */
		saving: boolean;
		/** Some change is being saved: every field waits. */
		busy: boolean;
		error?: string | null;
		onsave: (markupPercent: number | null) => Promise<boolean>;
		onundeclare: () => void;
	} = $props();

	const stored = $derived(piece.markupPercent === null ? '' : String(piece.markupPercent));
	let text = $state('');
	let refused = $state(false);
	let editing = $state(false);
	let keptSame = $state(false);

	// The field follows the stored markup until the player types in it.
	$effect(() => {
		if (!editing) text = stored;
	});

	const inputId = $derived(`armour-markup-${piece.item.replace(/[^a-zA-Z0-9]+/g, '-')}`);
	const askReplaced = $derived(
		piece.replacedAt !== null && piece.markupPercent !== null && !keptSame,
	);

	async function commit() {
		editing = false;
		const value = parseMarkup(text);
		if (value === undefined) {
			refused = true;
			return;
		}
		refused = false;
		if (value === piece.markupPercent) return;
		const saved = await onsave(value);
		if (!saved) text = stored;
	}

	function onkeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			(event.currentTarget as HTMLInputElement).blur();
		} else if (event.key === 'Escape') {
			text = stored;
			refused = false;
			editing = false;
			(event.currentTarget as HTMLInputElement).blur();
		}
	}
</script>

<li class="grid grid-cols-[minmax(0,1fr)_6rem_9rem] items-center gap-x-4 py-2.5" data-testid="armour-piece">
	<div class="min-w-0">
		<span class="block truncate text-sm text-text" title={piece.item}>{piece.item}</span>
		<span class="block text-xs text-text-tertiary">
			{pricingNote(piece)}
			{#if piece.declared}
				<span aria-hidden="true"> · </span>
				<button
					type="button"
					class="text-text-tertiary underline-offset-2 hover:text-text hover:underline disabled:opacity-50"
					disabled={busy}
					onclick={onundeclare}
				>
					Take off the list
				</button>
			{/if}
		</span>
	</div>
	<span class="text-right text-xs tabular-nums text-text-secondary">
		{piece.ttPed === null ? '' : `${formatPed(piece.ttPed)} TT`}
	</span>
	<div class="flex items-center justify-end gap-1.5">
		<label for={inputId} class="sr-only">Markup paid for {piece.item}</label>
		<input
			id={inputId}
			type="text"
			inputmode="decimal"
			class="w-20 rounded-md border bg-surface px-2 py-1 text-right text-sm tabular-nums text-text
				placeholder:text-text-tertiary focus:border-accent focus:outline-none disabled:opacity-60
				{refused ? 'border-error' : 'border-border/70'}"
			placeholder="TT"
			bind:value={text}
			disabled={busy && !saving}
			aria-invalid={refused}
			aria-describedby={refused || error ? `${inputId}-error` : undefined}
			oninput={() => (editing = true)}
			onblur={() => void commit()}
			{onkeydown}
		/>
		<span class="text-sm text-text-tertiary" aria-hidden="true">%</span>
	</div>
	{#if refused || error}
		<p id={`${inputId}-error`} class="col-span-full text-xs text-error" role="alert">
			{refused ? MARKUP_REFUSED : error}
		</p>
	{/if}
	{#if askReplaced && piece.replacedAt}
		<p class="col-span-full flex flex-wrap items-baseline gap-x-2 text-xs text-text-secondary" role="note">
			<span class="mt-1 inline-block h-1.5 w-1.5 shrink-0 rounded-full bg-warning" aria-hidden="true"></span>
			<span>
				Its TT went back up {formatSeenAt(piece.replacedAt)}: a new piece? Type what it cost above, or
			</span>
			<button
				type="button"
				class="font-medium text-accent hover:text-text"
				onclick={() => (keptSame = true)}
			>
				keep {markupText(piece.markupPercent ?? 100)}
			</button>
		</p>
	{/if}
</li>
