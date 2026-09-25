/**
 * The typed-command transport: the thin runtime under the generated
 * bindings (`commands.gen.ts`).
 *
 * Every typed command resolves with its declared payload or rejects
 * with the backend's serialised `ApiErrorPayload` (`kind` + `message`).
 * This wrapper maps that payload onto the thrown `ApiError` contract,
 * carrying the kind and the message verbatim.
 *
 * Every command first awaits startup readiness (`./readiness.svelte`): a
 * call made while the backend is still starting is held, not sent, and
 * dispatches once it is ready. This is the one place that wait lives, so no
 * feature retries or suppresses a startup `unavailable`.
 */

import { invoke } from '@tauri-apps/api/core';
import { ApiError } from './client';
import { API_ERROR_KINDS, type ApiErrorKind } from './commands.gen';
import { whenSubstrateReady } from './readiness.svelte';

/** The display message for the kinds that deliberately carry none. */
const MESSAGE_FOR_KIND: Partial<Record<ApiErrorKind, string>> = {
	internal: 'Internal Server Error',
	unavailable: 'backend substrate not ready',
};

function isContractKind(kind: string): kind is ApiErrorKind {
	return API_ERROR_KINDS.some((candidate) => candidate === kind);
}

export async function invokeCommand<T>(command: string, args: Record<string, unknown>): Promise<T> {
	await whenSubstrateReady();
	try {
		return await invoke<T>(command, args);
	} catch (raw) {
		if (raw && typeof raw === 'object' && 'kind' in raw && typeof raw.kind === 'string') {
			const { kind } = raw;
			const message = 'message' in raw && typeof raw.message === 'string' ? raw.message : undefined;
			if (isContractKind(kind)) {
				throw new ApiError(kind, message ?? MESSAGE_FOR_KIND[kind] ?? kind);
			}
			throw new ApiError('unknown', message ?? kind);
		}
		// A rejection outside the typed contract (an argument that failed
		// deserialisation, a missing command): surface it verbatim.
		throw new ApiError('unknown', String(raw));
	}
}
