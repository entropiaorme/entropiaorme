import type { SessionDefinition } from '$lib/api';
import { filterByQuery } from '$lib/view/search';

/** One catalogue order everywhere definitions are offered: readable,
 * case-insensitive, numeric-aware, and deterministic for equal names. */
export function compareDefinitions(a: SessionDefinition, b: SessionDefinition): number {
	return (
		a.name.localeCompare(b.name, undefined, { sensitivity: 'base', numeric: true }) ||
		a.id.localeCompare(b.id, undefined, { numeric: true })
	);
}

export function sortDefinitions(definitions: SessionDefinition[]): SessionDefinition[] {
	return definitions.toSorted(compareDefinitions);
}

export function filterDefinitions(
	definitions: SessionDefinition[],
	query: string,
): SessionDefinition[] {
	return filterByQuery(definitions, query, (definition) => definition.name);
}
