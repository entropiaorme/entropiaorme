import { describe, expect, it, vi } from 'vitest';
import { createCatalogueSearch } from './catalogueSearch';

const CATALOGUE = ['Sollomate Opalo', 'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)'];

describe('createCatalogueSearch', () => {
	it('loads the catalogue on the first query only and matches it locally', async () => {
		const load = vi.fn(async () => CATALOGUE);
		const search = createCatalogueSearch(load, (name) => name, 10);

		await expect(search('exceptional electric 15')).resolves.toEqual([CATALOGUE[1]]);
		await expect(search('opalo')).resolves.toEqual([CATALOGUE[0]]);
		expect(load).toHaveBeenCalledTimes(1);
	});

	it('answers a query with nothing to search without loading', async () => {
		const load = vi.fn(async () => CATALOGUE);
		const search = createCatalogueSearch(load, (name) => name, 10);

		await expect(search('  ')).resolves.toEqual([]);
		expect(load).not.toHaveBeenCalled();
	});

	it('caps the matches', async () => {
		const search = createCatalogueSearch(
			async () => CATALOGUE,
			(name) => name,
			1,
		);
		await expect(search('o')).resolves.toHaveLength(1);
	});

	it('tries the load again after it fails', async () => {
		const load = vi
			.fn<() => Promise<string[]>>()
			.mockRejectedValueOnce(new Error('offline'))
			.mockResolvedValueOnce(CATALOGUE);
		const search = createCatalogueSearch(load, (name) => name, 10);

		await expect(search('opalo')).rejects.toThrow('offline');
		await expect(search('opalo')).resolves.toEqual([CATALOGUE[0]]);
		expect(load).toHaveBeenCalledTimes(2);
	});
});
