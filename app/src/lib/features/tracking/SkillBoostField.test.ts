// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	getTrackingSnapshot: vi.fn(),
	setSessionConfig: vi.fn(async () => ({})),
	selectDefinition: vi.fn(async () => ({})),
	ApiError: class ApiError extends Error {},
}));
vi.mock('$lib/api', () => api);

import SkillBoostField from './SkillBoostField.svelte';

const field = () => screen.getByLabelText('Skill boost percent') as HTMLInputElement;

beforeEach(() => {
	vi.clearAllMocks();
	api.getTrackingSnapshot.mockResolvedValue({
		status: 'idle',
		sessionName: 'ARIS Dailies',
		skillBoostPercent: 50,
	});
});

describe('the skill boost field', () => {
	it('shows the boost in force', async () => {
		render(SkillBoostField);
		await waitFor(() => expect(field().value).toBe('50'));
	});

	it('declares a typed boost, carrying the session name through unchanged', async () => {
		render(SkillBoostField);
		await waitFor(() => expect(field().value).toBe('50'));
		await fireEvent.input(field(), { target: { value: '150' } });
		await fireEvent.blur(field());
		await waitFor(() => expect(api.setSessionConfig).toHaveBeenCalledWith('ARIS Dailies', 150));
	});

	it('withdraws the declaration when emptied, and declares unboosted play on a typed 0', async () => {
		render(SkillBoostField);
		await waitFor(() => expect(field().value).toBe('50'));
		await fireEvent.input(field(), { target: { value: '' } });
		await fireEvent.blur(field());
		await waitFor(() =>
			expect(api.setSessionConfig).toHaveBeenLastCalledWith('ARIS Dailies', null),
		);
		await fireEvent.input(field(), { target: { value: '0' } });
		await fireEvent.keyDown(field(), { key: 'Enter' });
		await waitFor(() => expect(api.setSessionConfig).toHaveBeenLastCalledWith('ARIS Dailies', 0));
	});

	it('stays editable while a session runs: it stamps each gain, not the session', async () => {
		api.getTrackingSnapshot.mockResolvedValue({ status: 'active', skillBoostPercent: null });
		render(SkillBoostField);
		await waitFor(() => expect(field().disabled).toBe(false));
		expect(field().value).toBe('');
	});

	it('says when the write is refused', async () => {
		api.setSessionConfig.mockRejectedValueOnce(new Error('No session to configure'));
		render(SkillBoostField);
		await waitFor(() => expect(field().value).toBe('50'));
		await fireEvent.input(field(), { target: { value: '75' } });
		await fireEvent.blur(field());
		await waitFor(() => expect(screen.getByText('No session to configure')).toBeTruthy());
	});
});
