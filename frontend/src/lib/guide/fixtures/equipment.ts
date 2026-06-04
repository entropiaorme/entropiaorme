import type { Equipment, EquipmentDetail } from '$lib/types';
import type { Hotbar, TrifectaSettings } from '$lib/types/settings';

/**
 * Inline demo data for the equipment surface guide-mode mount.
 *
 * The equipment guide uses a narrow inline fixture rather than the bundled
 * demo DB + `/demo/` namespace path because the surface walks a small, fixed
 * slice (a single representative loadout) that does not need the seeded
 * library or any derived-chain calibration to be useful.
 */

export const equipmentDemoLibrary: Equipment[] = [
	{
		id: '1',
		name: 'Jester D-1',
		type: 'weapon',
		amplifierName: null,
		costPerUse: 0.42,
		damageMin: 13.0,
		damageMax: 22.5,
		reloadSeconds: 2.5,
		isLimited: false,
		enrichmentLevel: 2,
	},
	{
		id: '2',
		name: 'Korss H400',
		type: 'weapon',
		amplifierName: 'Omegaton A104',
		costPerUse: 4.18,
		damageMin: 88.0,
		damageMax: 137.0,
		reloadSeconds: 4.2,
		isLimited: false,
		enrichmentLevel: 3,
	},
	{
		id: '3',
		name: 'CB14 Marksman',
		type: 'weapon',
		amplifierName: null,
		costPerUse: 1.35,
		damageMin: 32.0,
		damageMax: 48.0,
		reloadSeconds: 3.1,
		isLimited: true,
		enrichmentLevel: 1,
	},
	{
		id: '4',
		name: 'Vivo T1',
		type: 'healing',
		amplifierName: null,
		costPerUse: 0.18,
		damageMin: null,
		damageMax: null,
		reloadSeconds: 3.5,
		isLimited: false,
		enrichmentLevel: 2,
	},
	{
		id: '5',
		name: 'Animal Eye Oil',
		type: 'consumable',
		amplifierName: null,
		costPerUse: 0,
		damageMin: null,
		damageMax: null,
		reloadSeconds: null,
		isLimited: false,
		enrichmentLevel: 0,
	},
];

export const equipmentDemoDetails: Record<string, EquipmentDetail> = {
	'1': {
		id: '1',
		weapon: {
			catalogId: 'jester-d1',
			name: 'Jester D-1',
			decay: 0.18,
			ammoBurn: 0.24,
			markupPercent: 100,
			isLimited: false,
			damageEnhancers: 0,
		},
		amplifier: null,
		scope: null,
		absorber: null,
		costBreakdown: [
			{
				component: 'Jester D-1 decay',
				costPec: 0.18,
				markupMultiplier: 1.0,
				effectiveCostPec: 0.18,
			},
			{
				component: 'Jester D-1 ammo',
				costPec: 0.24,
				markupMultiplier: 1.0,
				effectiveCostPec: 0.24,
			},
		],
		totalCostPerUse: 0.42,
	},
	'2': {
		id: '2',
		weapon: {
			catalogId: 'korss-h400',
			name: 'Korss H400',
			decay: 0.398,
			ammoBurn: 1.62,
			markupPercent: 100,
			isLimited: false,
			damageEnhancers: 2,
		},
		amplifier: {
			catalogId: 'omegaton-a104',
			name: 'Omegaton A104',
			decay: 1.61,
			ammoBurn: 0.55,
			markupPercent: 100,
			isLimited: false,
		},
		scope: null,
		absorber: null,
		costBreakdown: [
			{
				component: 'Korss H400 decay',
				costPec: 0.398,
				markupMultiplier: 1.0,
				effectiveCostPec: 0.398,
			},
			{
				component: 'Korss H400 ammo',
				costPec: 1.62,
				markupMultiplier: 1.0,
				effectiveCostPec: 1.62,
			},
			{
				component: 'Omegaton A104 decay',
				costPec: 1.61,
				markupMultiplier: 1.0,
				effectiveCostPec: 1.61,
			},
			{
				component: 'Omegaton A104 ammo',
				costPec: 0.55,
				markupMultiplier: 1.0,
				effectiveCostPec: 0.55,
			},
		],
		totalCostPerUse: 4.18,
	},
};

export const equipmentDemoTrifecta: TrifectaSettings = {
	activePresetId: 'preset-default',
	activePresetName: 'Caboria hunt',
	presets: [
		{
			id: 'preset-default',
			name: 'Caboria hunt',
			smallWeaponId: 1,
			bigWeaponId: 2,
			healId: 4,
			ready: true,
			message: null,
		},
	],
	ready: true,
	message: null,
};

export const equipmentDemoHotbar: Hotbar = {
	'1': 1,
	'2': 2,
	'3': 4,
	'4': 5,
	'5': null,
	'6': null,
	'7': null,
	'8': null,
	'9': null,
	'0': null,
};
