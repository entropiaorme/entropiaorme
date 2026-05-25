export interface GameConnection {
	chatLogPath: string;
	chatLogValid: boolean;
	playerName: string;
}

export type MobTrackingMode = 'mob' | 'tag';

/** Hotbar slot mapping: key "1"-"9" → equipment_library ID or null */
export type Hotbar = Record<string, number | null>;

export interface TrifectaPreset {
	id: string;
	name: string;
	smallWeaponId: number | null;
	bigWeaponId: number | null;
	healId: number | null;
	ready: boolean;
	message: string | null;
}

export interface TrifectaSettings {
	activePresetId: string | null;
	activePresetName: string | null;
	presets: TrifectaPreset[];
	ready: boolean;
	message: string | null;
}

export interface AppSettings {
	gameConnection: GameConnection;
	hotbarHooksEnabled: boolean;
	repairOcrEnabled: boolean;
	endOfSessionArmourReminderEnabled: boolean;
	developerModeEnabled: boolean;
	mobTrackingMode: MobTrackingMode;
	mobTrackingTag: string;
	hotbar: Hotbar;
	trifecta: TrifectaSettings;
	lootFilterBlacklist: string[];
	dbPath: string;
	appVersion: string;
}
