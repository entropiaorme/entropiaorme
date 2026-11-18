/**
 * What the account readings recorded of one item within the last hour, for
 * a manual form to show beside its amount: a conversion beside the
 * conversion and recycling forms, a Trade Terminal sale beside the trade
 * form. Re-read on every account frame, so a record the readings make while
 * the form is open appears without reopening it.
 *
 * Each read carries the item it was asked for, and an answer for an item
 * other than the one now in the form is never shown. A failed read shows
 * nothing: the line is a reminder, and the form works the same without it.
 */
import { ACCOUNT_TOPIC } from '$lib/api/account';
import { getConversionFromAccount, getSaleFromAccount } from '$lib/api/inventory';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import type { AccountRecordKind, AccountRecordRead } from './fromAccount';

type Reader = (input: { itemName: string }) => Promise<AccountRecordRead | null>;

/** The backend's answer for each kind; none while it does not answer one. */
function readerFor(kind: AccountRecordKind): Reader | null {
	return kind === 'conversion' ? getConversionFromAccount : getSaleFromAccount;
}

interface RecordRead {
	itemName: string;
	record: AccountRecordRead | null;
}

export function createAccountRecord(kind: AccountRecordKind, itemName: () => string) {
	const asked = () => itemName().trim();
	const store = createSnapshotStore<RecordRead>(ACCOUNT_TOPIC, async () => {
		const name = asked();
		const read = readerFor(kind);
		if (name === '' || read === null) return { itemName: name, record: null };
		return { itemName: name, record: await read({ itemName: name }) };
	});

	const read = $derived(store.current?.itemName === asked() ? store.current : null);

	return {
		/** The record of the item now in the form, if any. */
		get record(): AccountRecordRead | null {
			return read?.record ?? null;
		},
		refresh(): Promise<void> {
			return store.hydrate();
		},
		/** Attach the account topic listener, then read; returns the detach. */
		connect(): () => void {
			return store.connect();
		},
	};
}

export type AccountRecord = ReturnType<typeof createAccountRecord>;
