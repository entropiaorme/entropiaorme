import type {
	ActivityOption,
	ActivityOptionsResult,
	ManualMobSuggestion,
	QuestHandInState,
	SessionDefinition,
} from '$lib/api';

export const OVERLAY_MENU_WINDOW_LABEL = 'overlay-menu';
export const OVERLAY_MENU_READY_EVENT = 'overlay-menu:ready';
export const OVERLAY_MENU_SHOW_EVENT = 'overlay-menu:show';
export const OVERLAY_MENU_HIDE_EVENT = 'overlay-menu:hide';
export const OVERLAY_MENU_SELECT_EVENT = 'overlay-menu:select';
export const OVERLAY_MENU_CLOSED_EVENT = 'overlay-menu:closed';
export const OVERLAY_MENU_INTERACT_EVENT = 'overlay-menu:interact';

export type OverlayMenuKind = 'definition' | 'mob' | 'activities' | 'questHandIn';

export type OverlayMenuState =
	| OverlayDefinitionMenuState
	| OverlayMobMenuState
	| OverlayActivitiesMenuState
	| OverlayQuestHandInMenuState;

export interface OverlayQuestHandInMenuState {
	kind: 'questHandIn';
	width: number;
	handIn: QuestHandInState;
}

/** The session picker: the authored definitions with the current
 * selection marked. Tapping another row switches to it; the selected
 * row is inert, because a session always runs under one. */
export interface OverlayDefinitionMenuState {
	kind: 'definition';
	width: number;
	definitions: {
		id: string;
		name: string;
		selected: boolean;
	}[];
}

/** The declared-mob suggestion menu: catalogue mobs typed ahead. */
export interface OverlayMobMenuState {
	kind: 'mob';
	width: number;
	query: string;
	loading: boolean;
	error: string | null;
	mobSuggestions: ManualMobSuggestion[];
}

/** The Activities control: the session's roster resolved against what
 * is in play, plus the facts nobody rostered. A tap switches (sealing
 * whatever was standing, whichever kind it was), a tap on a standing row
 * ends it, and the `+` co-activates. The free-text row appears only
 * where the definition opts into naming segments in play. */
export interface OverlayActivitiesMenuState {
	kind: 'activities';
	width: number;
	options: ActivityOption[];
	adHocSegments: boolean;
	/** No session is running, so the rows show what this session WILL
	 * offer and none of them can be declared yet. */
	idle: boolean;
	/** The name typed but not yet recorded, so a re-presented menu
	 * restores it. The model owns the buffer; the panel's input is a
	 * view of it, or a refused declaration would silently eat what was
	 * typed. */
	segmentDraft: string;
}

export type OverlayMenuSelection =
	| { kind: 'definition'; definitionId: string; selected: boolean }
	| { kind: 'mob'; species: string; maturity: string }
	| { kind: 'activities'; action: 'toggle'; key: string }
	| { kind: 'activities'; action: 'coActivate'; key: string }
	| { kind: 'activities'; action: 'declare'; label: string }
	| { kind: 'activities'; action: 'handIn'; key: string }
	| { kind: 'questHandIn'; action: 'completed' | 'cancelled' };

/** The widest label's rendered width in the overlay menus' font, for
 * sizing a satellite menu to its content. Falls back to a character
 * estimate where no 2D context is available (test environments). */
export function measureMenuTextWidth(
	labels: string[],
	font = '500 12px Inter, system-ui, sans-serif',
) {
	if (labels.length === 0) return 0;
	const canvas = document.createElement('canvas');
	const context = canvas.getContext('2d');
	if (!context) return labels.reduce((longest, label) => Math.max(longest, label.length * 8), 0);

	context.font = font;
	return labels.reduce((longest, label) => Math.max(longest, context.measureText(label).width), 0);
}

export const OVERLAY_MENU_MIN_WIDTH = 180;
export const OVERLAY_MENU_MAX_WIDTH = 340;
export const OVERLAY_MENU_MAX_HEIGHT = 220;

/** A menu's panel width: the anchor's width as the floor, the widest
 * label plus padding as the content, clamped to the satellite bounds. */
export function computeMenuWidth(minWidth: number, labels: string[], padding: number): number {
	const contentWidth = measureMenuTextWidth(labels);
	return Math.max(
		Math.ceil(minWidth),
		Math.min(
			OVERLAY_MENU_MAX_WIDTH,
			Math.max(OVERLAY_MENU_MIN_WIDTH, Math.ceil(contentWidth + padding)),
		),
	);
}

/** A menu's window height for its row count. */
export function computeMenuHeight(rows: number): number {
	return Math.min(OVERLAY_MENU_MAX_HEIGHT, Math.max(44, rows * 34 + 12));
}

/** Rows a menu state renders, which sets the satellite window's height
 * (the overlay sizes the window from this and the popup route sizes its
 * panel from the same count). Every kind falls back to one row: the
 * loading, error, and empty states each occupy exactly one line. The
 * Activities menu counts its free-text row as one more. */
export function menuRowCount(state: OverlayMenuState): number {
	if (state.kind === 'definition') return Math.max(1, state.definitions.length);
	if (state.kind === 'activities') {
		// The free-text row is one more line, and the empty state is one
		// line of its own.
		const entry = state.adHocSegments ? 1 : 0;
		return Math.max(1, state.options.length + entry);
	}
	if (state.kind === 'questHandIn') {
		// The waiting view can become a multi-item candidate without a new
		// parent-window show event. Reserve the satellite's existing maximum
		// height up front so that transition never clips the confirmation.
		return 7;
	}
	if (state.loading || state.error) return 1;
	return Math.max(1, state.mobSuggestions.length);
}

export function buildQuestHandInMenuState(
	anchorWidth: number,
	handIn: QuestHandInState,
): OverlayQuestHandInMenuState {
	return {
		kind: 'questHandIn',
		width: Math.max(300, Math.min(OVERLAY_MENU_MAX_WIDTH, Math.ceil(anchorWidth))),
		handIn,
	};
}

/** The session picker's menu state over the fetched definitions.
 * The width padding leaves room for the Selected badge beside a name. */
export function buildDefinitionMenuState(
	anchorWidth: number,
	definitions: SessionDefinition[],
	selectedId: string | null,
): OverlayDefinitionMenuState {
	const labels =
		definitions.length > 0
			? definitions.map((definition) => definition.name)
			: ['Sessions unavailable'];
	return {
		kind: 'definition',
		width: computeMenuWidth(anchorWidth, labels, 96),
		definitions: definitions.map((definition) => ({
			id: definition.id,
			name: definition.name,
			selected: definition.id === selectedId,
		})),
	};
}

/** The Activities menu's state over the fetched offerings. The extra
 * width padding leaves room for a row's status badge and the
 * co-activate button beside its name. */
export function buildActivitiesMenuState(
	anchorWidth: number,
	options: ActivityOptionsResult,
	idle: boolean,
	segmentDraft: string,
): OverlayActivitiesMenuState {
	const labels = options.options.map((option) => option.name);
	return {
		kind: 'activities',
		width: computeMenuWidth(anchorWidth, labels.length > 0 ? labels : ['Nothing to declare'], 108),
		options: options.options,
		adHocSegments: options.adHocSegments,
		idle,
		segmentDraft,
	};
}
