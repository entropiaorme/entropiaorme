import { describe, expect, it } from 'vitest';
import {
	createSearchIndex,
	filterByQuery,
	highlightMatches,
	isSearchQuery,
	matchesQuery,
	rankByQuery,
} from './search';

const WEAPONS = [
	'Sollomate Opalo',
	'Sollomate Opalo, FEN Edition',
	'Sollomate Opalo (L)',
	'EWE EP-1 Neutron (L)',
	'EWE EP-10 Self Defense',
	'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)',
	'Mayhem Electric Attack Nanochip 15, Perfected',
	'Kinetic Attack Nanochip 16',
	'Electrocution Chip 1 (L)',
	'Laser Rifle LR-15',
	'Café Pistol',
];

const byName = (name: string) => name;
const filter = (query: string) => filterByQuery(WEAPONS, query, byName);
const rank = (query: string) => rankByQuery(WEAPONS, query, byName);

describe('filterByQuery', () => {
	it('matches every word in any order, wherever it sits in the name', () => {
		expect(filter('Exceptional electric 15')).toEqual([
			'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)',
		]);
		expect(filter('15 electric exceptional')).toEqual([
			'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)',
		]);
	});

	it('requires every word to be present', () => {
		expect(filter('opalo neutron')).toEqual([]);
	});

	it('matches partial words, including inside a word', () => {
		expect(filter('elec')).toEqual([
			'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)',
			'Mayhem Electric Attack Nanochip 15, Perfected',
			'Electrocution Chip 1 (L)',
		]);
		expect(filter('nanochip')).toHaveLength(2);
		expect(filter('chip perf')).toHaveLength(2);
	});

	it('forgives one typo in a longer word', () => {
		expect(filter('exceptonal elecrtic')).toEqual([
			'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)',
		]);
	});

	it('matches numbers exactly, never by typo', () => {
		expect(filter('nanochip 16')).toEqual(['Kinetic Attack Nanochip 16']);
	});

	it('ignores case, accents and punctuation', () => {
		expect(filter('OPALO')).toHaveLength(3);
		expect(filter('cafe')).toEqual(['Café Pistol']);
		expect(filter('café')).toEqual(['Café Pistol']);
		expect(filter('mayhem 15, perfected')).toHaveLength(2);
	});

	it('searches text outside the Latin alphabet', () => {
		const pins = ['Лагерь Атрокс', 'Atrox Camp'];
		expect(filterByQuery(pins, 'атрокс', byName)).toEqual(['Лагерь Атрокс']);
		expect(filterByQuery(pins, 'atrox', byName)).toEqual(['Atrox Camp']);
	});

	it('splits letter-digit seams so a run-together model code still matches', () => {
		expect(filter('ep10')).toEqual(['EWE EP-10 Self Defense']);
		expect(filter('ep-10')).toEqual(['EWE EP-10 Self Defense']);
		expect(filter('lr15')).toEqual(['Laser Rifle LR-15']);
	});

	it('honours an exact phrase in quotes', () => {
		expect(filter('"chip 1"')).toEqual(['Electrocution Chip 1 (L)']);
		expect(filter('"chip 15"')).toEqual([
			'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)',
		]);
	});

	it('reads an unclosed quote as a phrase to the end, its last word still open', () => {
		expect(filter('"opalo, fen')).toEqual(['Sollomate Opalo, FEN Edition']);
		expect(filter('"opalo, fe')).toEqual(['Sollomate Opalo, FEN Edition']);
		expect(filter('"chip 1')).toEqual([
			'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)',
			'Electrocution Chip 1 (L)',
		]);
	});

	it('excludes whole words with a leading minus', () => {
		expect(filter('opalo -fen')).toEqual(['Sollomate Opalo', 'Sollomate Opalo (L)']);
		// `-L` drops limited items but leaves words merely starting with L.
		expect(filter('opalo -l')).toEqual(['Sollomate Opalo', 'Sollomate Opalo, FEN Edition']);
		expect(filter('laser -l')).toEqual(['Laser Rifle LR-15']);
	});

	it('excludes a quoted phrase', () => {
		expect(filter('opalo -"fen edition"')).toEqual(['Sollomate Opalo', 'Sollomate Opalo (L)']);
	});

	it('filters by exclusions alone', () => {
		expect(filter('-l')).not.toContain('Sollomate Opalo (L)');
		expect(filter('-l')).toContain('Sollomate Opalo');
	});

	it('keeps the original order', () => {
		expect(filter('opalo')).toEqual([
			'Sollomate Opalo',
			'Sollomate Opalo, FEN Edition',
			'Sollomate Opalo (L)',
		]);
	});

	it('treats a query with nothing searchable as no filter', () => {
		expect(filter('')).toEqual(WEAPONS);
		expect(filter('   ')).toEqual(WEAPONS);
		expect(filter('-')).toEqual(WEAPONS);
		expect(filter('""')).toEqual(WEAPONS);
		expect(filter('(,)')).toEqual(WEAPONS);
	});

	it('searches several fields, letting each word land in any field', () => {
		const quests = [
			{ name: 'Atrox Hunt', planet: 'Calypso' },
			{ name: 'Atrox Hunt', planet: 'Arkadia' },
		];
		const text = (q: (typeof quests)[number]) => [q.name, q.planet];
		expect(filterByQuery(quests, 'atrox calypso', text)).toEqual([quests[0]]);
		expect(filterByQuery(quests, 'hunt arkadia', text)).toEqual([quests[1]]);
	});

	it('never runs a word or a phrase across a field boundary', () => {
		const rows = [{ a: 'Foo', b: 'Bar' }];
		expect(filterByQuery(rows, 'foobar', (r) => [r.a, r.b])).toEqual([]);
		expect(filterByQuery(rows, '"foo bar"', (r) => [r.a, r.b])).toEqual([]);
		expect(filterByQuery(rows, 'foo bar', (r) => [r.a, r.b])).toEqual(rows);
	});

	it('skips absent fields', () => {
		const rows = [{ name: 'Pin', notes: null as string | null }];
		expect(filterByQuery(rows, 'pin', (r) => [r.name, r.notes])).toEqual(rows);
	});
});

describe('rankByQuery', () => {
	it('puts the closest match first', () => {
		expect(rank('chip 1')[0]).toBe('Electrocution Chip 1 (L)');
		expect(rank('opalo')[0]).toBe('Sollomate Opalo');
	});

	it('ranks a typo-free match above a typo match', () => {
		const names = ['Electric Fan', 'Eletcric Fan'];
		expect(rankByQuery(names, 'electric', byName)).toEqual(names);
	});

	it('ranks words in the typed order above reordered ones', () => {
		const names = ['Attack Mayhem', 'Mayhem Attack'];
		expect(rankByQuery(names, 'mayhem attack', byName)[0]).toBe('Mayhem Attack');
	});

	it('caps the result count', () => {
		expect(rankByQuery(WEAPONS, 'opalo', byName, 2)).toHaveLength(2);
		expect(rankByQuery(WEAPONS, '', byName, 2)).toEqual(WEAPONS.slice(0, 2));
	});

	it('still requires every word when the query is too long to rank', () => {
		const ranked = rank('exceptional mayhem electric attack chip 15 perfected');
		expect(ranked).toEqual(['Exceptional Mayhem Electric Attack Chip 15, Perfected (L)']);
		expect(rank('perfected 15 chip attack electric mayhem exceptional')).toEqual(ranked);
	});
});

describe('createSearchIndex', () => {
	it('reads each item once however many queries run', () => {
		let reads = 0;
		const index = createSearchIndex(WEAPONS, (name) => {
			reads += 1;
			return name;
		});
		index.filter('opalo');
		index.rank('chip');
		index.filter('ewe');
		expect(reads).toBe(WEAPONS.length);
	});
});

describe('matchesQuery', () => {
	it('answers for a single text', () => {
		expect(matchesQuery('Sollomate Opalo', 'opalo sollo')).toBe(true);
		expect(matchesQuery('Sollomate Opalo', 'opalo -sollomate')).toBe(false);
		expect(matchesQuery(['HP gain', 'Health'], 'health')).toBe(true);
		expect(matchesQuery('Anything', '')).toBe(true);
	});
});

describe('isSearchQuery', () => {
	it('is false for blank or punctuation-only input', () => {
		expect(isSearchQuery('')).toBe(false);
		expect(isSearchQuery(' - ')).toBe(false);
		expect(isSearchQuery('-fen')).toBe(true);
		expect(isSearchQuery('a')).toBe(true);
	});
});

describe('highlightMatches', () => {
	const marked = (text: string, query: string) =>
		highlightMatches(text, query)
			.filter((segment) => segment.match)
			.map((segment) => segment.text);

	it('marks each matched term', () => {
		expect(marked('Exceptional Mayhem Electric Attack Chip 15', 'exceptional electric 15')).toEqual(
			['Exceptional', 'Electric', '15'],
		);
	});

	it('reassembles to the original text', () => {
		const text = 'Café Pistol, SGA Edition';
		const joined = highlightMatches(text, 'cafe sga')
			.map((segment) => segment.text)
			.join('');
		expect(joined).toBe(text);
		expect(marked(text, 'cafe sga')).toEqual(['Café', 'SGA']);
	});

	it('marks the terms present even when others are not', () => {
		expect(marked('Atrox Hunt', 'atrox calypso')).toEqual(['Atrox']);
	});

	it('leaves excluded words unmarked and a blank query whole', () => {
		expect(marked('Sollomate Opalo', 'opalo -fen')).toEqual(['Opalo']);
		expect(highlightMatches('Sollomate Opalo', '')).toEqual([
			{ text: 'Sollomate Opalo', match: false },
		]);
	});
});
