/**
 * The equipment family: catalogue search, the library, and the
 * per-item detail. Thin wrappers over the generated typed commands,
 * plus the catalogue search, which matches each vocabulary's catalogue
 * locally through the shared search.
 */

import { createCatalogueSearch } from '$lib/view/catalogueSearch';
import type { EquipmentSearchHit, SearchKind } from './commands.gen';
import * as commands from './commands.gen';

/** An equipment catalogue entry. The two
 * optional fields are not part of the wire shape: the equipment page
 * reuses this type to seed its selection state from a stored detail,
 * which carries them. */
export type EquipmentSearchResult = EquipmentSearchHit & {
	markupPercent?: number;
	damageEnhancers?: number;
};

export const addToLibrary = commands.equipmentAdd;
export const getEquipmentLibrary = commands.equipmentLibrary;

/** Most catalogue matches a picker offers at once. */
const CATALOGUE_RESULT_LIMIT = 50;

const catalogueSearches = new Map<
	SearchKind,
	(query: string) => Promise<EquipmentSearchResult[]>
>();

/** One vocabulary's catalogue entries matching `q`, best first; a query
 * under two characters answers empty without loading anything. */
export async function searchEquipmentItems(
	q: string,
	type: SearchKind,
): Promise<EquipmentSearchResult[]> {
	if (q.trim().length < 2) return [];
	let search = catalogueSearches.get(type);
	if (!search) {
		search = createCatalogueSearch(
			() => commands.equipmentCatalogue(type),
			(entry) => entry.name,
			CATALOGUE_RESULT_LIMIT,
		);
		catalogueSearches.set(type, search);
	}
	return search(q);
}

export async function removeFromLibrary(id: string): Promise<void> {
	await commands.equipmentDelete(Number(id));
}

export async function updateLibrary(id: string, req: commands.EquipmentRequest) {
	return commands.equipmentUpdate(Number(id), req);
}

export async function getEquipmentDetail(id: string) {
	return commands.equipmentDetail(Number(id));
}
