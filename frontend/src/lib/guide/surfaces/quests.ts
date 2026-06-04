import { getDemoApi } from '../state.svelte';
import type { GuideSurface } from '../types';

/** Quests-surface demoApi method names (declared here for documentation). */
type QuestsDemoApi = {
	setView(view: 'quests' | 'playlists' | 'analytics'): void;
	openNewQuestModal(): void;
	closeNewQuestModal(): void;
	closePlaylistModal(): void;
};

function questsApi(): Partial<QuestsDemoApi> {
	return getDemoApi('quests') as Partial<QuestsDemoApi>;
}

export const questsSurface: GuideSurface = {
	id: 'quests',
	title: 'Quests',
	beforeStart(demoApi) {
		const api = demoApi as Partial<QuestsDemoApi>;
		api.setView?.('quests');
		api.closeNewQuestModal?.();
		api.closePlaylistModal?.();
	},
	steps: [
		{
			id: 'narrative-intro',
			prose: {
				title: 'Quests',
				body: [
					{ kind: 'p', text: 'The Quests tab helps with:' },
					{
						kind: 'ul',
						items: [
							'Cooldown timers.',
							'Automatically detecting quest start/completions, and automatically adding quest rewards to your ledger.',
							'Analysing cost/reward of completing a quest or a quest playlist.',
						],
					},
				],
				note: 'Note: Guide uses demo data.',
			},
		},
		{
			id: 'new-quest-form',
			anchor: () =>
				document.querySelector('[role="dialog"][aria-label="New Quest"]') as HTMLElement | null,
			prose: {
				title: 'Creating a quest',
				body: [
					{ kind: 'p', text: 'When creating a quest:' },
					{
						kind: 'ul',
						items: [
							'The name must match the in-game quest name. chat.log is read to automatically detect when a quest has been started/completed.',
							'Set up reward value/cooldown to automatically add that reward to your ledger upon completion, and track cooldown.',
							'Additional details are for your convenience.',
						],
					},
				],
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<QuestsDemoApi>;
				api.openNewQuestModal?.();
				await wait(500);
			},
			resetDemo() {
				questsApi().closeNewQuestModal?.();
			},
		},
		{
			id: 'playlists-overview',
			anchor: () =>
				document.querySelector<HTMLElement>('[data-guide-anchor="quests-playlists-view"]'),
			prose: {
				title: 'Playlists',
				body: 'By creating playlists, you can access them in the Quests dashboard widget to have them handy during gameplay, as well as analysing quest playlist rewards as one unit.',
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<QuestsDemoApi>;
				api.closeNewQuestModal?.();
				api.setView?.('playlists');
				await wait(500);
			},
			resetDemo() {
				questsApi().setView?.('quests');
			},
		},
		{
			id: 'analytics-tip',
			prose: {
				title: 'Quest analytics',
				body: 'Tip: When tracking a session, start tracking right before a quest/playlist, and finish tracking right after completing the quest/playlist. If the session matches a single quest/playlist, your overlay will suggest if you want to link that session to the given quest/playlist. That enables quest/playlist analytics.',
			},
			async play({ demoApi, wait }) {
				const api = demoApi as Partial<QuestsDemoApi>;
				api.setView?.('analytics');
				await wait(500);
			},
			resetDemo() {
				questsApi().setView?.('quests');
			},
		},
	],
};
