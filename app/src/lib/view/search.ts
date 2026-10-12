import uFuzzy from '@leeoniya/ufuzzy';

/**
 * The app's one text-search implementation. Every search box, picker, and
 * filter matches through here, so a query behaves the same wherever it is
 * typed:
 *
 * - **Words in any order, anywhere in the name.** `exceptional electric 15`
 *   finds "Exceptional Mayhem Electric Attack Chip 15, Perfected (L)": every
 *   word must appear, but not side by side and not in the order typed.
 * - **Partial words.** `elec` matches "Electric"; `chip` matches "Nanochip".
 * - **Forgiving of typos.** A word of five or more letters may carry one
 *   slip (a swapped, missing, extra, or wrong letter); shorter words allow
 *   less, and numbers always match exactly, so `15` never finds 16.
 * - **Case, accents, and punctuation are ignored.** `ep10` and `ep-10` both
 *   find "EP-10", `cafe` finds "Café", and `15,` is just `15`.
 * - **Exact phrases in quotes.** `"chip 1"` wants those whole words side by
 *   side, so it finds "Chip 1 (L)" but not "Chip 15". While the closing quote
 *   is still to come, the last word may be partial.
 * - **Exclusions with a leading minus.** `opalo -fen` drops the FEN Edition;
 *   `-L` drops limited items. An unquoted exclusion removes whole words only
 *   (so `-L` leaves "Laser" alone); a quoted one (`-"fen edition"`) removes
 *   a phrase.
 * - **Best match first** where results are a list of suggestions: matches in
 *   the typed order, at word starts, and without typos rank above the rest.
 *
 * Matching is delegated to uFuzzy; this module owns query parsing (quotes,
 * exclusions, letter-digit splitting), text folding, and the multi-field
 * haystack, so call sites never reach for `includes`.
 */

/** What an item is searched by: one string, or several fields (any may be absent). */
export type SearchText = string | readonly (string | null | undefined)[];

/** A run of displayed text, marked when it is part of a match. */
export interface HighlightSegment {
	text: string;
	match: boolean;
}

/** A query with its searchable content separated out. */
interface ParsedQuery {
	/** The positive terms as a uFuzzy needle; '' when there are none. */
	needle: string;
	/** The positive terms one by one (a phrase contributes its words). */
	terms: string[];
	/** Folded phrases an item must contain as whole words. */
	requiredPhrases: string[];
	/** Whole words that exclude an item. */
	excludedWords: string[];
	/** Folded phrases that exclude an item. */
	excludedPhrases: string[];
}

/**
 * Terms permuted for ranking out of order. uFuzzy runs one pass per
 * permutation (3! = 6 at most) and compiles fresh expressions for each as
 * the query changes; a longer query still requires every term, in any
 * order, but is returned unranked (it rarely leaves many matches to order).
 */
const OUT_OF_ORDER_TERMS = 3;

/**
 * Matches beyond this many are returned unranked: ordering thousands of
 * hits for a one- or two-letter query costs more than it tells.
 */
const RANK_LIMIT = 2000;

/**
 * Fields are joined into one haystack around a control character no query
 * can type, so a word never runs from one field into the next, while terms
 * may still be satisfied by different fields (`atrox calypso`).
 */
const FIELD_SEPARATOR = ' \u001f ';

const WORD_SPLIT = /[^\p{L}\p{N}']+/u;

// SingleError: one typo per term, with uFuzzy's own guards (an exact match
// for digits and one- or two-letter terms, a swap only for three or four
// letters).
const TYPO_TOLERANCE = {
	intraMode: 1,
	intraIns: 1,
	intraSub: 1,
	intraTrn: 1,
	intraDel: 1,
} as const;

/**
 * Text is folded to Latin before matching, so the fast Latin-alphabet
 * matcher serves almost every query. A query in another script falls back
 * to the universal one, whose Unicode expressions cost several times more
 * to compile.
 */
const latinMatcher = new uFuzzy({ ...TYPO_TOLERANCE });
const universalMatcher = new uFuzzy({
	...TYPO_TOLERANCE,
	unicode: true,
	interSplit: "[^\\p{L}\\d']+",
	intraSplit: '\\p{Ll}\\p{Lu}',
	interBound: '[^\\p{L}\\d]',
	intraBound: '\\p{L}\\d|\\d\\p{L}|\\p{Ll}\\p{Lu}',
	intraChars: "[\\p{L}\\d']",
	intraContr: "'\\p{L}{1,2}\\b",
});

function matcherFor(needle: string): uFuzzy {
	return /[^ -~]/.test(needle) ? universalMatcher : latinMatcher;
}

/**
 * Strip diacritics without changing the string's length, so match ranges
 * computed on the folded text index straight into the original.
 */
function fold(text: string): string {
	let out = '';
	for (const ch of text) {
		const bare = ch.normalize('NFD').replace(/\p{M}/gu, '');
		out += bare.length === ch.length ? bare : ch;
	}
	return uFuzzy.latinize(out);
}

/** Lower-cased, folded words of a text (punctuation dropped). */
function words(text: string): string[] {
	return fold(text)
		.toLocaleLowerCase()
		.split(WORD_SPLIT)
		.filter((word) => word !== '');
}

/**
 * Each field as its folded, lower-cased words between single spaces,
 * fields kept apart so a phrase cannot run from one into the next.
 */
function phraseForm(haystack: string): string {
	return haystack
		.split(FIELD_SEPARATOR)
		.map((field) => ` ${words(field).join(' ')} `)
		.join('|');
}

/** Split letter-digit seams so `ep10` reads as `ep 10` and finds "EP-10". */
function splitLetterDigit(term: string): string {
	return term.replace(/(\p{L})(\p{N})/gu, '$1 $2').replace(/(\p{N})(\p{L})/gu, '$1 $2');
}

const TOKEN = /(-?)"([^"]*)("?)|(\S+)/g;

function parseQuery(raw: string): ParsedQuery {
	const terms: string[] = [];
	const requiredPhrases: string[] = [];
	const excludedWords: string[] = [];
	const excludedPhrases: string[] = [];
	for (const token of fold(raw).matchAll(TOKEN)) {
		const [, minus, phrase, closing, bare] = token;
		if (phrase !== undefined) {
			const phraseWords = words(phrase);
			if (phraseWords.length === 0) continue;
			// A quote still being typed reads as a phrase to the end, its
			// last word open so the phrase keeps matching as it grows.
			const form = ` ${phraseWords.join(' ')}${closing ? ' ' : ''}`;
			if (minus) {
				excludedPhrases.push(form);
			} else {
				requiredPhrases.push(form);
				terms.push(...phraseWords);
			}
		} else if (bare.startsWith('-') && bare.length > 1) {
			excludedWords.push(...words(bare.slice(1)));
		} else {
			terms.push(...words(splitLetterDigit(bare)));
		}
	}
	return { needle: terms.join(' '), terms, requiredPhrases, excludedWords, excludedPhrases };
}

function isEmpty(parsed: ParsedQuery): boolean {
	return (
		parsed.terms.length === 0 &&
		parsed.excludedWords.length === 0 &&
		parsed.excludedPhrases.length === 0
	);
}

function haystackOf(text: SearchText): string {
	if (typeof text === 'string') return fold(text);
	return fold(text.filter((field): field is string => !!field).join(FIELD_SEPARATOR));
}

/** The word-level checks the fuzzy matcher cannot express: phrases and exclusions. */
function hasWordRules(parsed: ParsedQuery): boolean {
	return (
		parsed.requiredPhrases.length > 0 ||
		parsed.excludedWords.length > 0 ||
		parsed.excludedPhrases.length > 0
	);
}

function passesWordRules(haystack: string, parsed: ParsedQuery): boolean {
	if (parsed.excludedWords.length > 0) {
		const present = new Set(words(haystack));
		if (parsed.excludedWords.some((word) => present.has(word))) return false;
	}
	if (parsed.requiredPhrases.length > 0 || parsed.excludedPhrases.length > 0) {
		const form = phraseForm(haystack);
		if (!parsed.requiredPhrases.every((phrase) => form.includes(phrase))) return false;
		if (parsed.excludedPhrases.some((phrase) => form.includes(phrase))) return false;
	}
	return true;
}

/**
 * Indices of the matching haystack entries: best first when `ranked`,
 * otherwise in their original order. Null when the query has no
 * searchable content, which callers treat as "no filter".
 */
function matchIndices(haystack: string[], parsed: ParsedQuery, ranked: boolean): number[] | null {
	if (isEmpty(parsed)) return null;
	let indices: number[];
	if (parsed.terms.length === 0) {
		indices = haystack.map((_, i) => i);
	} else {
		// An out-of-order allowance below the term count makes uFuzzy AND
		// the terms one at a time and skip the permutations, which only
		// ranking needs; filtering always takes that cheaper road.
		const [idxs, info, order] = matcherFor(parsed.needle).search(
			haystack,
			parsed.needle,
			ranked ? OUT_OF_ORDER_TERMS : 1,
			ranked ? RANK_LIMIT : 0,
		);
		if (idxs === null) return null;
		if (ranked && info && order) {
			indices = order.map((i) => info.idx[i]);
		} else {
			indices = [...idxs].sort((a, b) => a - b);
		}
	}
	if (!hasWordRules(parsed)) return indices;
	return indices.filter((i) => passesWordRules(haystack[i], parsed));
}

/** A reusable search over a fixed item list; folds each item's text once. */
export interface SearchIndex<T> {
	/** Matching items in their original order; the list itself, unchanged,
	 * for a query with nothing to search. */
	filter(query: string): T[];
	/** Matching items best first, optionally capped; the list (or its head)
	 * for a query with nothing to search. */
	rank(query: string, limit?: number): T[];
}

export function createSearchIndex<T>(items: T[], text: (item: T) => SearchText): SearchIndex<T> {
	let haystack: string[] | null = null;
	const prepared = () => {
		haystack ??= items.map((item) => haystackOf(text(item)));
		return haystack;
	};
	return {
		filter(query) {
			const indices = matchIndices(prepared(), parseQuery(query), false);
			return indices === null ? items : indices.map((i) => items[i]);
		},
		rank(query, limit = Number.POSITIVE_INFINITY) {
			const indices = matchIndices(prepared(), parseQuery(query), true);
			if (indices === null) return limit >= items.length ? items : items.slice(0, limit);
			return indices.slice(0, limit).map((i) => items[i]);
		},
	};
}

/** Matching items in their original order; the list itself for an empty query. */
export function filterByQuery<T>(items: T[], query: string, text: (item: T) => SearchText): T[] {
	return createSearchIndex(items, text).filter(query);
}

/** Matching items best first, optionally capped; the list's head for an empty query. */
export function rankByQuery<T>(
	items: T[],
	query: string,
	text: (item: T) => SearchText,
	limit?: number,
): T[] {
	return createSearchIndex(items, text).rank(query, limit);
}

/** Whether one text matches; an empty query matches everything. */
export function matchesQuery(text: SearchText, query: string): boolean {
	const parsed = parseQuery(query);
	const indices = matchIndices([haystackOf(text)], parsed, false);
	return indices === null || indices.length > 0;
}

/** Whether the query has anything to search for (a lone `-` or `"` does not). */
export function isSearchQuery(query: string): boolean {
	return !isEmpty(parseQuery(query));
}

/**
 * The text split into matched and unmatched runs for display. Each search
 * term is located on its own, so a term the text does not contain (because
 * another field satisfied it) never blanks the highlights of the rest.
 */
export function highlightMatches(text: string, query: string): HighlightSegment[] {
	const parsed = parseQuery(query);
	if (parsed.terms.length === 0 || text === '') return [{ text, match: false }];
	const folded = fold(text);
	const marked = new Array<boolean>(text.length).fill(false);
	for (const term of parsed.terms) {
		const [, info, order] = matcherFor(term).search([folded], term, 0, Number.POSITIVE_INFINITY);
		if (!info || !order || order.length === 0) continue;
		const ranges = info.ranges[order[0]];
		for (let r = 0; r < ranges.length; r += 2) {
			for (let i = ranges[r]; i < ranges[r + 1]; i++) marked[i] = true;
		}
	}
	const segments: HighlightSegment[] = [];
	for (let i = 0; i < text.length; i++) {
		const last = segments.at(-1);
		if (last && last.match === marked[i]) last.text += text[i];
		else segments.push({ text: text[i], match: marked[i] });
	}
	return segments;
}
