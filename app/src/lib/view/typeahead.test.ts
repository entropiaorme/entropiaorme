import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createTypeahead } from './typeahead.svelte';

interface Item {
	id: number;
	name: string;
}

const alpha: Item = { id: 1, name: 'Alpha' };
const beta: Item = { id: 2, name: 'Beta' };

/** A manually resolvable search response, for driving in-flight races. */
function deferred<T>(): {
	promise: Promise<T>;
	resolve: (value: T) => void;
	reject: (err: unknown) => void;
} {
	let resolve!: (value: T) => void;
	let reject!: (err: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

/** Let a settled search's continuations (then/catch/finally) run. */
async function flush(): Promise<void> {
	await Promise.resolve();
	await Promise.resolve();
	await Promise.resolve();
}

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
});

describe('debounce and minimum length', () => {
	it('fires no search until the debounce elapses, then searches the trimmed query', () => {
		const search = vi.fn().mockResolvedValue([alpha]);
		const ta = createTypeahead<Item>({ search });
		ta.query = '  alp  ';
		expect(search).not.toHaveBeenCalled();

		vi.advanceTimersByTime(199);
		expect(search).not.toHaveBeenCalled();
		vi.advanceTimersByTime(1);
		expect(search).toHaveBeenCalledTimes(1);
		expect(search).toHaveBeenCalledWith('alp');
	});

	it('respects a custom debounceMs', () => {
		const search = vi.fn().mockResolvedValue([]);
		const ta = createTypeahead<Item>({ search, debounceMs: 500 });
		ta.query = 'alp';
		vi.advanceTimersByTime(499);
		expect(search).not.toHaveBeenCalled();
		vi.advanceTimersByTime(1);
		expect(search).toHaveBeenCalledTimes(1);
	});

	it('never searches a trimmed query under the default minLength of 2', () => {
		const search = vi.fn().mockResolvedValue([alpha]);
		const ta = createTypeahead<Item>({ search });
		ta.query = ' a ';
		vi.advanceTimersByTime(1000);
		expect(search).not.toHaveBeenCalled();
		expect(ta.results).toEqual([]);
	});

	it('respects a custom minLength', () => {
		const search = vi.fn().mockResolvedValue([]);
		const ta = createTypeahead<Item>({ search, minLength: 4 });
		ta.query = 'alp';
		vi.advanceTimersByTime(1000);
		expect(search).not.toHaveBeenCalled();
		ta.query = 'alph';
		vi.advanceTimersByTime(200);
		expect(search).toHaveBeenCalledWith('alph');
	});

	it('coalesces rapid typing into a single search for the final query', () => {
		const search = vi.fn().mockResolvedValue([alpha]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'al';
		vi.advanceTimersByTime(100);
		ta.query = 'alp';
		vi.advanceTimersByTime(100);
		ta.query = 'alph';
		vi.advanceTimersByTime(200);
		expect(search).toHaveBeenCalledTimes(1);
		expect(search).toHaveBeenCalledWith('alph');
	});

	it('clears stale results immediately when the query drops under minLength', async () => {
		const search = vi.fn().mockResolvedValue([alpha]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([alpha]);

		ta.query = 'a';
		expect(ta.results).toEqual([]);
		vi.advanceTimersByTime(1000);
		expect(search).toHaveBeenCalledTimes(1);
	});
});

describe('loading and results', () => {
	it('turns loading on when the search fires and off when it settles', async () => {
		const d = deferred<Item[]>();
		const search = vi.fn().mockReturnValue(d.promise);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		expect(ta.loading).toBe(false);

		vi.advanceTimersByTime(200);
		expect(ta.loading).toBe(true);

		d.resolve([alpha, beta]);
		await flush();
		expect(ta.loading).toBe(false);
		expect(ta.results).toEqual([alpha, beta]);
		expect(ta.error).toBeNull();
	});

	it('records the query a settled search answered, and forgets it on clear', async () => {
		const search = vi.fn().mockResolvedValue([]);
		const ta = createTypeahead<Item>({ search });
		expect(ta.settledQuery).toBeNull();
		ta.query = ' zeta ';
		vi.advanceTimersByTime(200);
		expect(ta.settledQuery).toBeNull();
		await flush();
		expect(ta.settledQuery).toBe('zeta');

		ta.clear();
		expect(ta.settledQuery).toBeNull();
	});

	it('records the query a failed search answered', async () => {
		const search = vi.fn().mockRejectedValue(new Error('down'));
		const ta = createTypeahead<Item>({ search });
		ta.query = 'zeta';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.settledQuery).toBe('zeta');
	});

	it('keeps the previous results visible while a follow-up search debounces', async () => {
		const search = vi.fn().mockResolvedValue([alpha]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([alpha]);

		ta.query = 'alph';
		expect(ta.results).toEqual([alpha]);
	});
});

describe('stale responses', () => {
	it('drops a response whose query no longer matches, even when it resolves last', async () => {
		const first = deferred<Item[]>();
		const second = deferred<Item[]>();
		const search = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
		const ta = createTypeahead<Item>({ search });

		ta.query = 'al';
		vi.advanceTimersByTime(200);
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		expect(search).toHaveBeenCalledTimes(2);

		second.resolve([beta]);
		await flush();
		expect(ta.results).toEqual([beta]);

		first.resolve([alpha]);
		await flush();
		expect(ta.results).toEqual([beta]);
	});

	it('accepts a response when only surrounding whitespace changed in the meantime', async () => {
		const d = deferred<Item[]>();
		const search = vi.fn().mockReturnValue(d.promise);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);

		ta.query = ' alp ';
		d.resolve([alpha]);
		await flush();
		expect(ta.results).toEqual([alpha]);
	});

	it('drops a response that lands after an item was selected mid-flight', async () => {
		const d = deferred<Item[]>();
		const search = vi.fn().mockReturnValue(d.promise);
		const ta = createTypeahead<Item>({ search, labelOf: (i) => i.name });
		ta.query = 'be';
		vi.advanceTimersByTime(200);

		ta.select(beta);
		d.resolve([alpha, beta]);
		await flush();
		expect(ta.results).toEqual([]);
		expect(ta.selected).toEqual(beta);
		expect(ta.loading).toBe(false);
	});
});

describe('errors', () => {
	it('sets error to a rejected Error message and empties the results', async () => {
		const search = vi
			.fn()
			.mockResolvedValueOnce([alpha])
			.mockRejectedValueOnce(new Error('backend away'));
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([alpha]);

		ta.query = 'alph';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.error).toBe('backend away');
		expect(ta.results).toEqual([]);
		expect(ta.loading).toBe(false);
	});

	it('stringifies a non-Error rejection', async () => {
		const search = vi.fn().mockRejectedValue('plain failure');
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.error).toBe('plain failure');
	});

	it('clears the error once a later search succeeds', async () => {
		const search = vi.fn().mockRejectedValueOnce(new Error('boom')).mockResolvedValueOnce([alpha]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.error).toBe('boom');

		ta.query = 'alph';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.error).toBeNull();
		expect(ta.results).toEqual([alpha]);
	});

	it('ignores a stale rejection: no error from a query already superseded', async () => {
		const first = deferred<Item[]>();
		const search = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValueOnce([beta]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'al';
		vi.advanceTimersByTime(200);
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([beta]);

		first.reject(new Error('too late'));
		await flush();
		expect(ta.error).toBeNull();
		expect(ta.results).toEqual([beta]);
	});
});

describe('selection', () => {
	it('select snaps the query via labelOf, clears results and cancels pending work', async () => {
		const search = vi.fn().mockResolvedValue([alpha, beta]);
		const ta = createTypeahead<Item>({ search, labelOf: (i) => i.name });
		ta.query = 'al';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([alpha, beta]);

		ta.query = 'alp';
		ta.select(alpha);
		expect(ta.selected).toEqual(alpha);
		expect(ta.query).toBe('Alpha');
		expect(ta.results).toEqual([]);
		expect(ta.error).toBeNull();

		vi.advanceTimersByTime(1000);
		expect(search).toHaveBeenCalledTimes(1);
	});

	it('select leaves the query unchanged when no labelOf is configured', () => {
		const ta = createTypeahead<Item>({ search: vi.fn().mockResolvedValue([]) });
		ta.query = 'al';
		ta.select(alpha);
		expect(ta.query).toBe('al');
	});

	it('suppresses searching while an item is selected', () => {
		const search = vi.fn().mockResolvedValue([]);
		const ta = createTypeahead<Item>({ search, labelOf: (i) => i.name });
		ta.select(alpha);
		ta.query = 'something else';
		vi.advanceTimersByTime(1000);
		expect(search).not.toHaveBeenCalled();
		expect(ta.results).toEqual([]);
	});

	it('clear resets selection, query, results and error, and searching resumes', async () => {
		const search = vi.fn().mockResolvedValue([beta]);
		const ta = createTypeahead<Item>({ search, labelOf: (i) => i.name });
		ta.select(alpha);
		ta.clear();
		expect(ta.selected).toBeNull();
		expect(ta.query).toBe('');
		expect(ta.results).toEqual([]);
		expect(ta.error).toBeNull();

		ta.query = 'be';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([beta]);
	});

	it('clear cancels a pending debounce', () => {
		const search = vi.fn().mockResolvedValue([]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'al';
		ta.clear();
		vi.advanceTimersByTime(1000);
		expect(search).not.toHaveBeenCalled();
	});
});

describe('refresh', () => {
	it('re-runs the search for the unchanged query after the debounce', async () => {
		const search = vi.fn().mockResolvedValue([alpha]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		await flush();
		expect(search).toHaveBeenCalledTimes(1);

		ta.refresh();
		expect(search).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(200);
		expect(search).toHaveBeenCalledTimes(2);
		expect(search).toHaveBeenLastCalledWith('alp');
	});

	it('drops an in-flight response even though its query still matches', async () => {
		const first = deferred<Item[]>();
		const search = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValueOnce([beta]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);

		// The search's behaviour changed under the same query: the response
		// computed under the old behaviour must not land.
		ta.refresh();
		first.resolve([alpha]);
		await flush();
		expect(ta.results).toEqual([]);
		expect(ta.loading).toBe(true);

		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([beta]);
		expect(ta.loading).toBe(false);
	});

	it('just clears when the current query is under minLength', () => {
		const search = vi.fn().mockResolvedValue([alpha]);
		const ta = createTypeahead<Item>({ search });
		ta.refresh();
		vi.advanceTimersByTime(1000);
		expect(search).not.toHaveBeenCalled();
		expect(ta.results).toEqual([]);
	});
});

describe('cancel', () => {
	it('blanks results, loading and error while keeping the query', async () => {
		const search = vi.fn().mockResolvedValueOnce([alpha]).mockRejectedValueOnce(new Error('boom'));
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([alpha]);

		ta.cancel();
		expect(ta.results).toEqual([]);
		expect(ta.loading).toBe(false);
		expect(ta.error).toBeNull();
		expect(ta.query).toBe('alp');
	});

	it('cancels a pending debounce and drops an in-flight response', async () => {
		const pending = createTypeahead<Item>({ search: vi.fn().mockResolvedValue([alpha]) });
		pending.query = 'alp';
		pending.cancel();
		vi.advanceTimersByTime(1000);
		expect(pending.results).toEqual([]);

		const d = deferred<Item[]>();
		const search = vi.fn().mockReturnValue(d.promise);
		const inFlight = createTypeahead<Item>({ search });
		inFlight.query = 'alp';
		vi.advanceTimersByTime(200);
		inFlight.cancel();
		d.resolve([alpha]);
		await flush();
		expect(inFlight.results).toEqual([]);
		expect(inFlight.loading).toBe(false);
	});

	it('searching resumes on the next query change', async () => {
		const search = vi.fn().mockResolvedValue([beta]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'alp';
		ta.cancel();
		ta.query = 'alph';
		vi.advanceTimersByTime(200);
		await flush();
		expect(ta.results).toEqual([beta]);
	});
});

describe('teardown', () => {
	it('destroy cancels a pending debounce', () => {
		const search = vi.fn().mockResolvedValue([]);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'al';
		ta.destroy();
		vi.advanceTimersByTime(1000);
		expect(search).not.toHaveBeenCalled();
	});

	it('destroy drops an in-flight response instead of writing state after teardown', async () => {
		const d = deferred<Item[]>();
		const search = vi.fn().mockReturnValue(d.promise);
		const ta = createTypeahead<Item>({ search });
		ta.query = 'al';
		vi.advanceTimersByTime(200);

		ta.destroy();
		d.resolve([alpha]);
		await flush();
		expect(ta.results).toEqual([]);

		const rejecting = createTypeahead<Item>({ search: () => Promise.reject(new Error('late')) });
		rejecting.query = 'al';
		vi.advanceTimersByTime(200);
		rejecting.destroy();
		await flush();
		expect(rejecting.error).toBeNull();
	});
});

describe('fill', () => {
	it('puts text in the box without searching for it', async () => {
		const search = vi.fn(async () => [alpha, beta]);
		const picker = createTypeahead<Item>({ search, debounceMs: 0, minLength: 1 });

		picker.fill('Shrapnel');
		await vi.advanceTimersByTimeAsync(50);
		await flush();

		expect(picker.query).toBe('Shrapnel');
		expect(search).not.toHaveBeenCalled();
		expect(picker.results).toEqual([]);
		expect(picker.loading).toBe(false);
	});

	it('drops a search already in flight, so no dropdown arrives after it', async () => {
		const pending = deferred<Item[]>();
		const search = vi.fn(() => pending.promise);
		const picker = createTypeahead<Item>({ search, debounceMs: 0, minLength: 1 });

		picker.query = 'Al';
		await vi.advanceTimersByTimeAsync(5);
		picker.fill('Shrapnel');
		pending.resolve([alpha]);
		await flush();

		expect(picker.query).toBe('Shrapnel');
		expect(picker.results).toEqual([]);
	});
});
