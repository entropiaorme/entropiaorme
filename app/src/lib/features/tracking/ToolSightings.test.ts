// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { ToolSighting } from '$lib/api';
import ToolSightings from './ToolSightings.svelte';
import type { ToolSightingModel } from './toolSightingModel.svelte';

function weapon(overrides: Partial<ToolSighting> = {}): ToolSighting {
	return {
		id: 't1',
		sighted: { kind: 'bundled', name: 'Sollomate Opalo', tool: 'weapon', shots: 212 },
		firstSeen: 1000,
		lastSeen: 2000,
		settled: null,
		...overrides,
	};
}

function unknown(overrides: Partial<ToolSighting> = {}): ToolSighting {
	return {
		id: 't2',
		sighted: { kind: 'unknown', text: 'Teleport Chip' },
		firstSeen: 1500,
		lastSeen: 1500,
		settled: null,
		...overrides,
	};
}

function model(
	sightings: ToolSighting[],
	overrides: Partial<ToolSightingModel> = {},
): ToolSightingModel {
	return {
		sightings,
		ended: true,
		error: null,
		notice: null,
		busy: null,
		offer: null,
		refresh: vi.fn(),
		followRunning: vi.fn(),
		turnOn: vi.fn(async () => {}),
		add: vi.fn(async () => {}),
		dismiss: vi.fn(async () => {}),
		leave: vi.fn(async () => {}),
		undo: vi.fn(async () => {}),
		dismissError: vi.fn(),
		...overrides,
	};
}

describe('the detected tools section', () => {
	it('is absent when nothing was read', () => {
		render(ToolSightings, { props: { model: model([]) } });
		expect(screen.queryByTestId('tool-sightings')).toBeNull();
	});

	it('shows a tool with what happened and settles it in one action', async () => {
		const shown = weapon();
		const m = model([shown]);
		render(ToolSightings, { props: { model: m } });
		expect(screen.getByText('Detected tools')).toBeTruthy();
		expect(screen.getByText('Sollomate Opalo')).toBeTruthy();
		expect(screen.getByText('Held for 212 shots, costed without attachments')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Add to equipment' }));
		expect(m.add).toHaveBeenCalledWith(shown);
		await fireEvent.click(screen.getByRole('button', { name: 'Leave' }));
		expect(m.leave).toHaveBeenCalledWith(shown);
	});

	it('asks once more before setting a real tool aside, in place', async () => {
		const shown = weapon();
		const m = model([shown]);
		render(ToolSightings, { props: { model: m } });
		await fireEvent.click(screen.getByRole('button', { name: 'Not a tool I track' }));
		expect(m.dismiss).not.toHaveBeenCalled();
		const confirm = within(screen.getByRole('group', { name: 'Confirm for Sollomate Opalo' }));
		expect(
			screen.getByText('It will no longer be costed when held. Not a tool I track?'),
		).toBeTruthy();
		await fireEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
		expect(m.dismiss).not.toHaveBeenCalled();
		expect(screen.getByRole('button', { name: 'Add to equipment' })).toBeTruthy();

		await fireEvent.click(screen.getByRole('button', { name: 'Not a tool I track' }));
		await fireEvent.click(
			within(screen.getByRole('group', { name: 'Confirm for Sollomate Opalo' })).getByRole(
				'button',
				{ name: 'Not a tool I track' },
			),
		);
		expect(m.dismiss).toHaveBeenCalledWith(shown);
	});

	it('offers to turn on a turned-off entry an addition linked to', async () => {
		const m = model([weapon({ settled: { kind: 'added', created: false } })], {
			offer: { id: '9', label: 'Sollomate Opalo' },
		});
		render(ToolSightings, { props: { model: m } });
		await fireEvent.click(screen.getByRole('button', { name: 'Turn Sollomate Opalo on' }));
		expect(m.turnOn).toHaveBeenCalled();
	});

	it('offers a name read only what fits it, as easily as the rest', async () => {
		const shown = unknown();
		const m = model([shown]);
		render(ToolSightings, { props: { model: m } });
		expect(screen.getByText('“Teleport Chip”')).toBeTruthy();
		expect(screen.getByText('Seen on the tool bar')).toBeTruthy();
		const actions = within(screen.getByRole('group', { name: 'Settle “Teleport Chip”' }));
		expect(actions.queryByRole('button', { name: 'Add to equipment' })).toBeNull();
		expect(actions.getAllByRole('button').map((b) => b.textContent?.trim())).toEqual([
			'Not a tool I track',
			'Leave',
		]);
		await fireEvent.click(actions.getByRole('button', { name: 'Not a tool I track' }));
		expect(m.dismiss).toHaveBeenCalledWith(shown);
		await fireEvent.click(actions.getByRole('button', { name: 'Leave' }));
		expect(m.leave).toHaveBeenCalledWith(shown);
	});

	it('does not offer adding while the session runs', () => {
		render(ToolSightings, { props: { model: model([weapon()], { ended: false }) } });
		expect(screen.queryByRole('button', { name: 'Add to equipment' })).toBeNull();
		expect(screen.getByRole('button', { name: 'Leave' })).toBeTruthy();
	});

	it('collapses a settled one to a quiet line with its undo', async () => {
		const settled = weapon({ settled: { kind: 'added', created: true } });
		const m = model([settled]);
		render(ToolSightings, { props: { model: m } });
		expect(screen.getByText('Added to equipment')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Leave' })).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Undo for Sollomate Opalo' }));
		expect(m.undo).toHaveBeenCalledWith(settled);
	});

	it('holds every action while one is being saved, and shows the notice and the error', () => {
		render(ToolSightings, {
			props: {
				model: model([weapon(), unknown()], {
					busy: 't1',
					notice: 'Sollomate Opalo added to equipment.',
					error: 'Already left.',
				}),
			},
		});
		for (const button of screen.getAllByRole('button', { name: 'Leave' })) {
			expect((button as HTMLButtonElement).disabled).toBe(true);
		}
		expect(screen.getByRole('status').textContent).toContain('Sollomate Opalo added to equipment.');
		expect(screen.getByText('Already left.')).toBeTruthy();
	});
});
