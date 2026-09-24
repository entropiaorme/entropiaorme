/**
 * The session-facet view model: the session a run is an instance of (and
 * the name it writes) plus the skill boost, and the writes that move
 * them. What the play COUNTS TOWARD is the Activities control's, not a
 * facet control's; see `activitiesModel`.
 *
 * The facets are independent by construction, so each control writes only
 * its own value and carries the other through unchanged. A facet may be
 * edited live only where its stamp grain is finer than the session: the
 * boost stamps each skill gain, so it floats; the session (and the name
 * it writes) is session-grain, so the backend fixes it once a session
 * runs (409) and correction is a post-hoc move. The model respects these
 * rather than duplicating them.
 *
 * The satellite-window plumbing (anchors, popup lifecycle) stays with the
 * overlay route; this model owns the state and the writes.
 */

import { ApiError } from '$lib/api';

export interface SessionFacetsDeps {
	/** The facets currently in force, as the snapshot reports them;
	 * `definitionId` is the selected session (stringified id). */
	readFacets: () => {
		name: string | null;
		definitionId: string | null;
		boost: number | null;
	};
	/** Whether a session is running (gates the session lock). */
	isSessionActive: () => boolean;
	/** Re-read the snapshot after a successful write. */
	refresh: () => Promise<unknown>;
	/** Full-state facet write: a null clears its facet. */
	setSessionConfig: (name: string | null, boost: number | null) => Promise<unknown>;
	/** Select the session the next run starts under; the backend
	 * writes the name facet with it. */
	selectDefinition: (id: number) => Promise<unknown>;
}

function describe(error: unknown, fallback: string): string {
	return error instanceof ApiError || error instanceof Error ? error.message : fallback;
}

export function createSessionFacets(deps: SessionFacetsDeps) {
	// The session selection's in-flight guard (the chip disables
	// while a selection write lands).
	let savingDefinition = $state(false);

	// The boost's edit buffer. The boost edits live: a pill expiring is a
	// genuine change worth recording, and the session keeps the latest
	// declaration.
	let boostDraft = $state('');
	let savingBoost = $state(false);

	// One channel for every facet write failure, so a refusal is surfaced
	// beside the controls rather than swallowed. The Activities control
	// borrows it for its own popup failures, since the strip has one
	// place to show a message.
	let facetError = $state<string | null>(null);

	/** The boost currently in force, as a write wants it. Reading the
	 * snapshot (not the draft) keeps a name write from moving the boost as
	 * a side effect. Three-state: null withdraws the declaration, 0
	 * declares deliberately-unboosted play, a positive number declares a
	 * magnitude. */
	function currentBoost(): number | null {
		const value = deps.readFacets().boost;
		return value !== null && value !== undefined && value >= 0 ? value : null;
	}

	async function write(name: string | null, boost: number | null) {
		await deps.setSessionConfig(name, boost);
		await deps.refresh();
	}

	/** Select the session for the next run; the backend writes the name
	 * facet with it. There is no withdrawal: a run always records under
	 * a session, so the picker only ever switches between them. */
	async function selectDefinition(id: string) {
		savingDefinition = true;
		facetError = null;
		try {
			await deps.selectDefinition(Number(id));
			await deps.refresh();
		} catch (error) {
			facetError = describe(error, 'Failed to select the session');
		}
		savingDefinition = false;
	}

	async function commitBoost() {
		// The empty field and a typed 0 are DIFFERENT declarations: empty
		// withdraws (claims nothing), 0 declares deliberately-unboosted
		// play, which is the baseline a boost's effect is measured
		// against. Anything unparseable or negative falls back to a
		// withdrawal rather than inventing a magnitude.
		const trimmed = boostDraft.trim();
		const parsed = trimmed ? Number.parseInt(trimmed, 10) : Number.NaN;
		const next = Number.isFinite(parsed) && parsed >= 0 ? parsed : null;
		if (next === currentBoost()) {
			// Normalise the buffer even when nothing moved, so a stray
			// "abc" or " 50 " does not linger as if it were persisted.
			boostDraft = next === null ? '' : String(next);
			return;
		}
		savingBoost = true;
		facetError = null;
		try {
			await write(deps.readFacets().name, next);
		} catch (error) {
			facetError = describe(error, 'Failed to set skill boost');
		}
		savingBoost = false;
		boostDraft = next === null ? '' : String(next);
	}

	return {
		get savingDefinition() {
			return savingDefinition;
		},
		get boostDraft() {
			return boostDraft;
		},
		set boostDraft(value: string) {
			boostDraft = value;
		},
		get savingBoost() {
			return savingBoost;
		},
		/** Whether the session (and the name it writes) may still be
		 * set. Both are session-grain, so a live edit could only rewrite
		 * the whole session's history: they are fixed once a session runs,
		 * and correction is a post-hoc move on the session record. The
		 * boost has no such flag because its grain is finer than the
		 * session, so it always edits. */
		get definitionEditable() {
			return !deps.isSessionActive();
		},
		get facetError() {
			return facetError;
		},

		/** Keep the boost buffer in step with the persisted value while the
		 * user is not editing it. A persisted 0 renders as "0", not as the
		 * empty field: it is a declaration, not the absence of one. */
		syncBoostDraft() {
			if (savingBoost) return;
			const persisted = deps.readFacets().boost;
			boostDraft =
				persisted !== null && persisted !== undefined && persisted >= 0 ? String(persisted) : '';
		},

		selectDefinition,
		commitBoost,
	};
}
