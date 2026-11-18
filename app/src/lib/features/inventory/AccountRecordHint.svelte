<script lang="ts">
	/**
	 * Beside a manual form's amount: what the account readings recorded of
	 * the same item within the last hour (a conversion, or a Trade Terminal
	 * sale), so the player does not enter it a second time. Nothing while
	 * there is none, while it is being read, or once the hour has passed. It
	 * never stops the form.
	 *
	 * Announced when it appears, and carries `id` so the amount it sits
	 * beside can be described by it. The held amount in the form follows the
	 * inventory, which re-reads when the readings record something.
	 */
	import { onMount, untrack } from 'svelte';
	import { createAccountRecord } from './accountRecord.svelte';
	import {
		type AccountRecordKind,
		accountRecordSentence,
		isRecentFromAccount,
		RECENT_FROM_ACCOUNT_MS,
	} from './fromAccount';

	let {
		kind,
		itemName,
		id,
	}: {
		kind: AccountRecordKind;
		itemName: string;
		/** For the amount's `aria-describedby`. */
		id: string;
	} = $props();

	// The kind is fixed for the form's life: a form asks about one thing.
	const recent = untrack(() => createAccountRecord(kind, () => itemName));

	// Read again whenever the form moves to another item; the first read is
	// the connection's, made once the listener is attached.
	let lastAsked: string | null = null;
	$effect(() => {
		const name = itemName;
		untrack(() => {
			if (lastAsked === null) {
				lastAsked = name;
				return;
			}
			if (name === lastAsked) return;
			lastAsked = name;
			void recent.refresh();
		});
	});

	onMount(() => recent.connect());

	// The backend judged the hour when it answered; a form left open past
	// it judges again, so the line goes when the hour does.
	let clock = $state(new Date());
	$effect(() => {
		const record = recent.record;
		if (!record) return;
		const left = Date.parse(record.recordedAt) + RECENT_FROM_ACCOUNT_MS - Date.now();
		if (left <= 0) {
			clock = new Date();
			return;
		}
		const timer = setTimeout(() => (clock = new Date()), left + 1000);
		return () => clearTimeout(timer);
	});

	const shown = $derived.by(() => {
		const record = recent.record;
		if (!record) return null;
		// The answer is fresher than a clock read before it arrived.
		const now = new Date(Math.max(clock.getTime(), Date.parse(record.recordedAt)));
		return isRecentFromAccount(record.recordedAt, now) ? record : null;
	});
</script>

{#if shown}
	<p
		{id}
		role="status"
		class="text-xs leading-relaxed text-warning"
		data-testid="account-record-hint"
	>
		{accountRecordSentence(kind, itemName.trim(), shown)}
	</p>
{/if}
