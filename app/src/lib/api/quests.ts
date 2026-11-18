/**
 * The quests family: quest CRUD, lifecycle verbs, and
 * the curated analytics rows. Thin wrappers over the generated typed
 * commands; argument shaping (string ids onto numeric commands) only.
 */

import type { Quest, QuestFamily, QuestFamilyInput, QuestInput } from './commands.gen';
import * as commands from './commands.gen';

export const getQuests = commands.questsList;
export const getQuestAnalytics = commands.questsAnalytics;
export const getUnresolvedQuestRewards = commands.questRewardsUnresolved;
export const reviewQuestReward = commands.questRewardReview;
export const createQuest = commands.questCreate;
export const getQuestFamilies = commands.questFamiliesList;
export const getQuestCatalogue = commands.questCatalogue;
export const createQuestFamily = commands.questFamilyCreate;

export async function updateQuestFamily(id: string, data: QuestFamilyInput): Promise<QuestFamily> {
	return commands.questFamilyUpdate(Number(id), data);
}

export async function deleteQuestFamily(id: string): Promise<void> {
	await commands.questFamilyDelete(Number(id));
}

export async function getQuest(id: string): Promise<Quest> {
	return commands.questGet(Number(id));
}

export async function updateQuest(id: string, data: QuestInput): Promise<Quest> {
	return commands.questUpdate(Number(id), data);
}

export async function deleteQuest(id: string): Promise<void> {
	await commands.questDelete(Number(id));
}

export async function startQuest(id: string): Promise<Quest> {
	return commands.questStart(Number(id));
}

export async function completeQuest(id: string): Promise<Quest> {
	return commands.questComplete(Number(id));
}

export const beginQuestHandIn = commands.questHandInBegin;
export const getQuestHandInState = commands.questHandInState;
export const waitForNextQuestHandInClump = commands.questHandInWait;
export const cancelQuestHandIn = commands.questHandInCancel;
export const confirmQuestHandIn = commands.questHandInConfirm;

export async function cancelQuest(id: string, undoReward = false): Promise<Quest> {
	return commands.questCancel(Number(id), undoReward);
}

/** Give up a quest in progress: no reward, and its slot stays held until
 * the next mission can be collected. */
export async function abandonQuest(id: string): Promise<Quest> {
	return commands.questAbandon(Number(id));
}
