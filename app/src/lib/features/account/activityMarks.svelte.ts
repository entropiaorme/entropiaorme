/**
 * What is new to the player in what the account did on its own: how far
 * they have seen it, and how many events came after. The sidebar's
 * Inventory icon and the Inventory's History tab carry a mark while any
 * did; opening "From your account" moves the watermark, which is kept in
 * the database, so the marks hold across restarts and windows.
 *
 * One store for the main window, connected by the shell that shows the
 * sidebar and read by any page in it. Kept current by the account topic:
 * every record the readings make and every proposal found or decided
 * announces itself there, and so does a watermark that moved. The
 * account commands are refused to every other window, so nothing else
 * connects it.
 */
import { ACCOUNT_TOPIC, getActivityMarks, markActivitySeen } from '$lib/api/account';
import type { AccountActivityMarks } from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';

function createActivityMarks() {
	const store = createSnapshotStore<AccountActivityMarks>(ACCOUNT_TOPIC, getActivityMarks);
	let connections = 0;
	let detach: (() => void) | null = null;

	return {
		/** The latest marks; `null` before the first read answers, or when
		 * there is no account connection to read. */
		get current(): AccountActivityMarks | null {
			return store.current;
		},
		/** Whether anything from the account is new to the player. */
		get anyNew(): boolean {
			return (store.current?.unseen ?? 0) > 0;
		},
		/** Attach the topic listener and read; shared, so the last detach
		 * of every connection stops listening. */
		connect(): () => void {
			connections += 1;
			if (connections === 1) detach = store.connect();
			let closed = false;
			return () => {
				if (closed) return;
				closed = true;
				connections -= 1;
				if (connections === 0) {
					detach?.();
					detach = null;
				}
			};
		},
		/** The player has seen what the account did up to `through`. The
		 * watermark only moves forward; a failure leaves the marks as they
		 * are, to be cleared the next time the space is open. */
		async seen(through: string): Promise<void> {
			try {
				await markActivitySeen({ through });
			} catch {
				return;
			}
			await store.hydrate();
		},
	};
}

export const activityMarks = createActivityMarks();
