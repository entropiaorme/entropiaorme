// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// The failure surface: the reason's sentence and remedy, a restart, and the
// logged detail copied for a report. The shell commands and the clipboard are
// the only seams.
const seams = vi.hoisted(() => ({
	restartApp: vi.fn(async () => {}),
	getVersion: vi.fn(async () => '0.3.0'),
	writeText: vi.fn(async () => {}),
}));

vi.mock('$lib/api/shell', () => ({ restartApp: seams.restartApp }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: seams.getVersion }));

import StartupFailure from './StartupFailure.svelte';

const failure = { reason: 'game_data_unavailable' as const, detail: 'snapshot at /x is empty' };

beforeEach(() => {
	seams.restartApp.mockClear();
	seams.writeText.mockReset();
	seams.writeText.mockResolvedValue(undefined);
	Object.defineProperty(navigator, 'clipboard', {
		value: { writeText: seams.writeText },
		configurable: true,
	});
});

describe('StartupFailure', () => {
	it('says what went wrong and what to do, without the logged detail', () => {
		render(StartupFailure, { failure });
		expect(screen.getByRole('alert')).toBeTruthy();
		expect(screen.getByText("EntropiaOrme couldn't start")).toBeTruthy();
		expect(screen.getByText('The game data bundled with EntropiaOrme is missing.')).toBeTruthy();
		expect(screen.getByText('Reinstalling EntropiaOrme restores it.')).toBeTruthy();
		expect(screen.queryByText(/snapshot at/)).toBeNull();
	});

	it('restarts the app', async () => {
		render(StartupFailure, { failure });
		await fireEvent.click(screen.getByRole('button', { name: 'Restart' }));
		expect(seams.restartApp).toHaveBeenCalledOnce();
	});

	it('copies the version, reason, and detail for a report', async () => {
		render(StartupFailure, { failure });
		await fireEvent.click(screen.getByRole('button', { name: 'Copy details' }));
		await waitFor(() => expect(screen.getByRole('button', { name: 'Copied' })).toBeTruthy());
		expect(seams.writeText).toHaveBeenCalledWith(
			[
				'EntropiaOrme 0.3.0 could not start.',
				'Reason: game_data_unavailable',
				'Detail: snapshot at /x is empty',
			].join('\n'),
		);
	});

	it('says so when the clipboard refuses', async () => {
		seams.writeText.mockRejectedValue(new Error('denied'));
		render(StartupFailure, { failure });
		await fireEvent.click(screen.getByRole('button', { name: 'Copy details' }));
		await waitFor(() => expect(screen.getByText('The clipboard is unavailable.')).toBeTruthy());
	});
});
