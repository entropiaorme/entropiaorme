import { describe, expect, it } from 'vitest';
import type { ActivityHistoryEntry } from '$lib/api/commands.gen';
import {
	type ConversionDraft,
	type ConvertibleItem,
	derivedFromLabel,
	draftFromEntry,
	emptyDraft,
	emptyInput,
	emptyOutput,
	excessOverHeld,
	inputKind,
	lastConversionFor,
	previewConversion,
	suggestedTt,
	toConversionRequest,
	worthIsShown,
} from './conversionDraft';

const vouchers: ConvertibleItem = {
	name: 'Hyperion Daily Voucher',
	assetId: null,
	heldQty: 210,
	unitTt: 0,
	paidPerUnit: 0,
	origins: [{ label: 'ARIS Dailies', quantity: 210 }],
};
const boxes: ConvertibleItem = {
	name: 'Halloween Strongbox',
	assetId: null,
	heldQty: 145,
	unitTt: 0,
	paidPerUnit: 0,
	origins: [{ label: 'Halloween Mayhem 2026', quantity: 145 }],
};
const farmed: ConvertibleItem = {
	name: 'Component X',
	assetId: null,
	heldQty: 300,
	unitTt: 0.2,
	paidPerUnit: 0,
	origins: [{ label: 'Mining', quantity: 300 }],
};
const items = [vouchers, boxes, farmed];

function handIn(): ConversionDraft {
	return {
		inputs: [
			emptyInput({ itemName: vouchers.name, quantity: 80 }),
			emptyInput({
				itemName: 'Component Y',
				quantity: 120,
				ttValue: 25,
				ttEdited: true,
				markup: 5,
			}),
			emptyInput({
				itemName: 'Component Z',
				quantity: 140,
				ttValue: 25,
				ttEdited: true,
				markup: 5,
			}),
		],
		outputs: [emptyOutput({ itemName: 'Hyperion Harness', ttValue: 50, ttEdited: true })],
		pedAmount: null,
		showPed: false,
	};
}

describe('what a line asks for', () => {
	it('treats an item the app can see as held and one it cannot as bought', () => {
		expect(inputKind(emptyInput({ itemName: vouchers.name }), items)).toBe('held');
		expect(inputKind(emptyInput({ itemName: 'Component Y' }), items)).toBe('bought');
	});

	it('lets the player say these particular ones were bought', () => {
		expect(inputKind(emptyInput({ itemName: farmed.name, bought: true }), items)).toBe('bought');
	});

	it('only asks what a line is worth when there is more than one source to divide', () => {
		const single = handIn();
		expect(worthIsShown(single, items)).toBe(false);

		const two = handIn();
		two.inputs[1] = emptyInput({ itemName: farmed.name, quantity: 120 });
		expect(worthIsShown(two, items)).toBe(true);
	});

	it("fills a known item's TT from its quantity", () => {
		expect(suggestedTt(farmed.name, 100, items)).toBe(20);
		expect(suggestedTt('Unknown Thing', 100, items)).toBeNull();
	});

	it('flags how far a quantity runs past what is held', () => {
		expect(excessOverHeld(emptyInput({ itemName: vouchers.name, quantity: 250 }), items)).toBe(40);
		expect(excessOverHeld(emptyInput({ itemName: vouchers.name, quantity: 80 }), items)).toBe(0);
	});
});

describe('the outcome sentence', () => {
	it("says where a hand-in's proceeds go and what must be recovered first", () => {
		expect(previewConversion(handIn(), items)).toEqual({
			ready: true,
			sentence:
				"You'll hold 1 Hyperion Harness. Anything it makes above 60.00, after fees, goes to ARIS Dailies.",
		});
	});

	it('settles a PED result immediately', () => {
		const draft = emptyDraft({ name: boxes.name, assetId: null });
		draft.inputs[0].quantity = 100;
		draft.outputs = [];
		draft.showPed = true;
		draft.pedAmount = 100;

		expect(previewConversion(draft, items).sentence).toBe(
			'+100.00 PED goes to Halloween Mayhem 2026.',
		);
	});

	it('names a TT gain as counting now', () => {
		const draft = emptyDraft({ name: boxes.name, assetId: null });
		draft.inputs[0].quantity = 100;
		draft.outputs = [emptyOutput({ itemName: 'Pumpkin Blade', ttValue: 30, ttEdited: true })];

		expect(previewConversion(draft, items).sentence).toBe(
			"+30.00 PED of TT counts now, for Halloween Mayhem 2026. You'll hold 1 Pumpkin Blade. Anything it makes above its 30.00 TT, after fees, goes to Halloween Mayhem 2026.",
		);
	});

	it('pays declared worth first and the rest to the lines left blank', () => {
		const draft = handIn();
		draft.inputs[1] = emptyInput({ itemName: farmed.name, quantity: 125, markup: 5 });
		draft.inputs.pop();
		draft.outputs[0].ttValue = 25;

		expect(previewConversion(draft, items).sentence).toBe(
			"You'll hold 1 Hyperion Harness. Anything it makes above its 25.00 TT, after fees, pays 5.00 to Mining first, then the rest goes to ARIS Dailies.",
		);
	});

	it('says so when nothing came from tracked play', () => {
		const draft = emptyDraft();
		draft.inputs[0] = emptyInput({ itemName: 'Thing', quantity: 1, ttValue: 5, ttEdited: true });
		draft.outputs = [emptyOutput({ itemName: 'Other', ttValue: 5, ttEdited: true })];

		expect(previewConversion(draft, items).sentence).toBe(
			"You'll hold 1 Other. None of this came from tracked play, so selling it won't credit an activity.",
		);
	});

	it('holds back until every line is complete', () => {
		const draft = handIn();
		draft.inputs[1].ttEdited = false;
		draft.inputs[1].ttValue = null;
		expect(previewConversion(draft, items)).toEqual({ ready: false, sentence: null });

		const noResult = emptyDraft({ name: boxes.name, assetId: null });
		noResult.inputs[0].quantity = 100;
		expect(previewConversion(noResult, items).ready).toBe(false);
	});
});

describe('the recorded request', () => {
	it('sends each line as what it turned out to be', () => {
		expect(toConversionRequest(handIn(), items, '2026-06-02')).toEqual({
			inputs: [
				{ kind: 'held', itemName: vouchers.name, quantity: 80, worth: null },
				{ kind: 'bought', itemName: 'Component Y', quantity: 120, ttValue: 25, markupPaid: 5 },
				{ kind: 'bought', itemName: 'Component Z', quantity: 140, ttValue: 25, markupPaid: 5 },
			],
			outputs: [{ itemName: 'Hyperion Harness', quantity: 1, ttValue: 50, holding: 'loot' }],
			pedAmount: null,
			convertedAt: '2026-06-02',
		});
	});

	it('records an asset result as one whole holding', () => {
		const draft = handIn();
		draft.outputs[0].holding = 'asset';
		draft.outputs[0].quantity = 7;

		expect(toConversionRequest(draft, items, null).outputs[0].quantity).toBe(1);
	});

	it('ignores a worth left behind when the field is no longer shown', () => {
		const draft = handIn();
		draft.inputs[0].markup = 9;

		expect(toConversionRequest(draft, items, null).inputs[0]).toMatchObject({ worth: null });
	});
});

describe('repeating an earlier conversion', () => {
	const entry = {
		kind: 'conversion',
		itemised: true,
		undone: false,
		pedAmount: null,
		conversionLines: [
			{
				side: 'input',
				itemName: vouchers.name,
				quantity: 80,
				ttValue: 0,
				acquisition: 'held',
				markup: null,
				holding: null,
			},
			{
				side: 'input',
				itemName: 'Component Y',
				quantity: 120,
				ttValue: 25,
				acquisition: 'bought',
				markup: 5,
				holding: null,
			},
			{
				side: 'output',
				itemName: 'Hyperion Harness',
				quantity: 1,
				ttValue: 50,
				acquisition: null,
				markup: null,
				holding: 'asset',
			},
		],
	} as ActivityHistoryEntry;

	it('finds the newest conversion that used the item', () => {
		expect(lastConversionFor(vouchers.name, [entry])).toBe(entry);
		expect(lastConversionFor('Nothing', [entry])).toBeNull();
		expect(lastConversionFor(vouchers.name, [{ ...entry, undone: true }])).toBeNull();
	});

	it('rebuilds the draft ready to confirm', () => {
		const draft = draftFromEntry(entry, items);

		expect(previewConversion(draft, items).ready).toBe(true);
		expect(draft.outputs[0]).toMatchObject({ itemName: 'Hyperion Harness', holding: 'asset' });
		expect(draft.inputs[1]).toMatchObject({ itemName: 'Component Y', ttValue: 25, markup: 5 });
	});
});

describe('the origin line under a holding', () => {
	it('names a whole count and drops a proportional one', () => {
		expect(derivedFromLabel([{ itemName: 'Voucher', quantity: 80 }])).toBe('From 80 Voucher');
		expect(derivedFromLabel([{ itemName: 'Strongbox', quantity: 33.4 }])).toBe('From Strongbox');
		expect(derivedFromLabel([])).toBeNull();
	});
});
