<script lang="ts">
	import type { ProtectionCandidateSession } from '$lib/api';
	import { formatPed } from '$lib/utils/format';
	import {
		formatDay,
		formatSessionStart,
		groupState,
		projectedShare,
		type SessionGroup,
	} from './armourRecording';
	import type { ArmourRecording } from './armourRecordingModel.svelte';

	let { recording }: { recording: ArmourRecording } = $props();

	// Opening a session type is the rare path, so every type starts closed.
	let openGroups = $state<Set<string>>(new Set());

	const cost = $derived(recording.costPed);
	const since = $derived(
		recording.candidates?.since ?? recording.sessions[0]?.startedAt ?? null,
	);
	const sinceLabel = $derived(
		recording.stream.kind === 'unlimited' ? 'last repair' : 'last reading',
	);

	function toggleOpen(key: string): void {
		const next = new Set(openGroups);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		openGroups = next;
	}

	function groupHits(group: SessionGroup): number {
		return group.sessions
			.filter((session) => recording.chosenIds.has(session.sessionId))
			.reduce((sum, session) => sum + session.hitCount, 0);
	}

	function share(hits: number): string | null {
		if (cost === null || recording.hits === 0 || hits === 0) return null;
		return formatPed(projectedShare(cost, hits, recording.hits));
	}

	function sessionLabel(session: ProtectionCandidateSession, group: SessionGroup): string {
		const when = formatSessionStart(session.startedAt);
		return session.sessionName && session.sessionName !== group.label
			? `${when} · ${session.sessionName}`
			: when;
	}

	function indeterminate(node: HTMLInputElement, value: boolean) {
		node.indeterminate = value;
		return {
			update(next: boolean) {
				node.indeterminate = next;
			},
		};
	}

	function startFrom(event: Event): void {
		recording.startFrom((event.currentTarget as HTMLSelectElement).value);
	}
</script>

<div class="flex flex-col gap-1.5" data-testid="armour-session-picker">
	<div class="flex items-center justify-between gap-4 text-[10px] text-white/40">
		{#if recording.sessions.length > 1}
			<label class="since-control">
				<span class="sr-only">Cover sessions from</span>
				<select
					class="since-select"
					value={recording.sessions[recording.selection.start]?.sessionId}
					onchange={startFrom}
				>
					{#each recording.sessions as session, index (session.sessionId)}
						<option value={session.sessionId}>
							{index === 0 && since !== null
								? `Since ${formatDay(since)}${recording.candidates?.since ? ` (${sinceLabel})` : ''}`
								: `From ${formatSessionStart(session.startedAt)}`}
						</option>
					{/each}
				</select>
			</label>
		{:else}
			<span></span>
		{/if}
		<span class="tabular-nums">
			{recording.chosen.length} {recording.chosen.length === 1 ? 'session' : 'sessions'} · {recording.hits}
			{recording.hits === 1 ? 'hit' : 'hits'}
		</span>
	</div>

	<ul class="picker-list" aria-label="Sessions this cost covers">
		{#each recording.groups as group (group.key)}
			{@const state = groupState(group, recording.chosenIds)}
			{@const open = openGroups.has(group.key)}
			{@const hits = groupHits(group)}
			<li>
				<div class="picker-row">
					<input
						type="checkbox"
						class="picker-check"
						checked={state === 'all'}
						use:indeterminate={state === 'some'}
						aria-label={`Include ${group.label}`}
						onchange={() => recording.toggleGroup(group)}
					/>
					<button
						type="button"
						class="picker-name"
						aria-expanded={open}
						onclick={() => toggleOpen(group.key)}
						title={open ? 'Hide the sessions' : 'Choose single sessions'}
					>
						<span class="truncate {state === 'none' ? 'text-white/35' : 'text-white/80'}">{group.label}</span>
						<span class="picker-chevron" class:picker-chevron-open={open} aria-hidden="true">›</span>
					</button>
					<span class="picker-meta">{group.sessions.length} {group.sessions.length === 1 ? 'session' : 'sessions'}</span>
					<span class="picker-meta w-14 text-right">{hits} {hits === 1 ? 'hit' : 'hits'}</span>
					<span class="picker-share">{share(hits) ?? ''}</span>
				</div>
				{#if open}
					<ul class="pl-5">
						{#each group.sessions as session (session.sessionId)}
							{@const ticked = recording.chosenIds.has(session.sessionId)}
							<li class="picker-row">
								<input
									type="checkbox"
									class="picker-check"
									checked={ticked}
									aria-label={`Include the session of ${formatSessionStart(session.startedAt)}`}
									onchange={() => recording.toggleSession(session.sessionId)}
								/>
								<span class="flex-1 truncate {ticked ? 'text-white/65' : 'text-white/30'}">{sessionLabel(session, group)}</span>
								<span class="picker-meta w-14 text-right">{session.hitCount} {session.hitCount === 1 ? 'hit' : 'hits'}</span>
								<span class="picker-share">{ticked ? (share(session.hitCount) ?? '') : ''}</span>
							</li>
						{/each}
					</ul>
				{/if}
			</li>
		{/each}

		{#if recording.earlierOpen}
			{#each recording.earlierGroups as group (group.key)}
				<li class="pt-1">
					<p class="px-0.5 pb-0.5 text-[10px] text-white/30">{group.label}, before the {sinceLabel}</p>
					<ul>
						{#each group.sessions as session (session.sessionId)}
							{@const ticked = recording.chosenIds.has(session.sessionId)}
							<li class="picker-row">
								<input
									type="checkbox"
									class="picker-check"
									checked={ticked}
									aria-label={`Include the earlier session of ${formatSessionStart(session.startedAt)}`}
									onchange={() => recording.toggleEarlier(session.sessionId)}
								/>
								<span class="flex-1 truncate {ticked ? 'text-white/65' : 'text-white/30'}">{sessionLabel(session, group)}</span>
								{#if session.covered}<span class="picker-meta">recorded</span>{/if}
								<span class="picker-meta w-14 text-right">{session.hitCount} {session.hitCount === 1 ? 'hit' : 'hits'}</span>
								<span class="picker-share">{ticked ? (share(session.hitCount) ?? '') : ''}</span>
							</li>
						{/each}
					</ul>
				</li>
			{/each}
		{/if}
	</ul>

	{#if recording.earlier.length > 0}
		<button type="button" class="earlier-toggle" aria-expanded={recording.earlierOpen} onclick={recording.toggleEarlierOpen}>
			{recording.earlierOpen ? 'Hide earlier sessions' : 'Include sessions from before the last repair'}
		</button>
	{/if}
</div>

<style>
	.since-control {
		position: relative;
		display: inline-flex;
		align-items: center;
	}

	.since-select {
		appearance: none;
		color-scheme: dark;
		background: transparent;
		border: none;
		padding: 0 0.9rem 0 0;
		color: rgba(255, 255, 255, 0.45);
		font-size: 10px;
		cursor: pointer;
		background-image: linear-gradient(45deg, transparent 50%, currentColor 50%),
			linear-gradient(135deg, currentColor 50%, transparent 50%);
		background-position:
			calc(100% - 5px) 55%,
			calc(100% - 2px) 55%;
		background-size: 3px 3px;
		background-repeat: no-repeat;
	}

	.since-select:hover,
	.since-select:focus-visible {
		color: rgba(255, 255, 255, 0.8);
		outline: none;
	}

	.picker-list {
		max-height: 13.5rem;
		overflow-y: auto;
		border-top: 1px solid rgba(255, 255, 255, 0.07);
		border-bottom: 1px solid rgba(255, 255, 255, 0.07);
		padding: 0.25rem 0;
	}

	.picker-row {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		min-height: 1.5rem;
		font-size: 11px;
	}

	.picker-check {
		width: 0.75rem;
		height: 0.75rem;
		flex-shrink: 0;
		accent-color: var(--color-accent);
		cursor: pointer;
	}

	.picker-name {
		display: inline-flex;
		min-width: 0;
		flex: 1;
		align-items: center;
		gap: 0.35rem;
		text-align: left;
		cursor: pointer;
	}

	.picker-name:focus-visible {
		outline: 1px solid var(--color-accent);
		outline-offset: 2px;
	}

	.picker-chevron {
		color: rgba(255, 255, 255, 0.3);
		transition: transform 120ms ease;
	}

	.picker-chevron-open {
		transform: rotate(90deg);
	}

	.picker-meta {
		flex-shrink: 0;
		font-size: 10px;
		font-variant-numeric: tabular-nums;
		color: rgba(255, 255, 255, 0.35);
	}

	.picker-share {
		width: 3.25rem;
		flex-shrink: 0;
		text-align: right;
		font-size: 10px;
		font-variant-numeric: tabular-nums;
		color: rgba(255, 255, 255, 0.6);
	}

	.earlier-toggle {
		align-self: flex-start;
		font-size: 10px;
		color: rgba(255, 255, 255, 0.3);
		cursor: pointer;
	}

	.earlier-toggle:hover,
	.earlier-toggle:focus-visible {
		color: rgba(255, 255, 255, 0.65);
		outline: none;
	}

	@media (prefers-reduced-motion: reduce) {
		.picker-chevron {
			transition: none;
		}
	}
</style>
