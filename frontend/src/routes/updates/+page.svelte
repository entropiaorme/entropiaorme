<script lang="ts">
	import { onMount } from 'svelte';
	import { Button, Toggle } from '$lib/components';
	import {
		autoUpdateEnabled,
		availableUpdate,
		checkForUpdate,
		downloadProgress,
		downloadUpdate,
		getUpdateChannel,
		installUpdate,
		setAutoUpdateEnabled,
		updateError,
		updatePhase,
	} from '$lib/updater';

	let currentVersion = $state('');
	let channel = $state('stable');

	onMount(async () => {
		channel = await getUpdateChannel();
		try {
			// Resolve the running version from the Tauri shell when available.
			const { getVersion } = await import('@tauri-apps/api/app');
			currentVersion = await getVersion();
		} catch {
			// Outside Tauri (e.g. a prerender pass): leave blank.
		}
	});

	// Percent of the download done, or null when the server announced no total
	// (chunked transfer) so the UI shows an indeterminate bar.
	let downloadPercent = $derived.by(() => {
		const p = $downloadProgress;
		if (!p || !p.contentLength) return null;
		return Math.min(100, Math.round((p.downloaded / p.contentLength) * 100));
	});

	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	async function onToggleAutoUpdate(checked: boolean) {
		await setAutoUpdateEnabled(checked);
	}
</script>

<div class="page">
	<header class="head">
		<span class="eyebrow">Updates</span>
		<h1 class="title">Keep EntropiaOrme up to date</h1>
		<p class="lede">
			EntropiaOrme checks <code>entropiaorme.com</code> for new versions. Updates are signed and
			verified before they install, and nothing about you is sent in the check.
		</p>
	</header>

	<section class="panel">
		<div class="panel-row">
			<div class="meta">
				<span class="meta-label">Installed version</span>
				<span class="meta-value">{currentVersion || '-'}</span>
			</div>
			<div class="meta">
				<span class="meta-label">Channel</span>
				<span class="meta-value channel">{channel === 'beta' ? 'Beta' : 'Stable'}</span>
			</div>
		</div>

		<div class="divider"></div>

		<div class="status">
			{#if $updatePhase === 'checking'}
				<div class="status-line">
					<span class="spinner" aria-hidden="true"></span>
					<span>Checking for updates…</span>
				</div>
			{:else if $updatePhase === 'available'}
				<div class="status-head">
					<span class="signal" aria-hidden="true"></span>
					<div>
						<p class="status-title">
							Version {$availableUpdate?.version} is available.
						</p>
						<p class="status-sub">You're on {currentVersion || 'an earlier version'}.</p>
					</div>
				</div>
				{#if $availableUpdate?.notes}
					<div class="notes" aria-label="Release notes">{$availableUpdate.notes}</div>
				{/if}
				<div class="actions">
					<Button variant="primary" onclick={downloadUpdate}>Download update</Button>
				</div>
			{:else if $updatePhase === 'downloading'}
				<div class="status-head">
					<span class="signal" aria-hidden="true"></span>
					<p class="status-title">Downloading version {$availableUpdate?.version}…</p>
				</div>
				<div
					class="bar"
					class:indeterminate={downloadPercent === null}
					role="progressbar"
					aria-valuemin="0"
					aria-valuemax="100"
					aria-valuenow={downloadPercent ?? undefined}
				>
					<span class="bar-fill" style={downloadPercent !== null ? `width:${downloadPercent}%` : ''}></span>
				</div>
				<p class="status-sub">
					{#if downloadPercent !== null && $downloadProgress}
						{downloadPercent}% · {formatBytes($downloadProgress.downloaded)}
					{:else if $downloadProgress}
						{formatBytes($downloadProgress.downloaded)} downloaded
					{:else}
						Starting download…
					{/if}
				</p>
			{:else if $updatePhase === 'ready'}
				<div class="status-head">
					<span class="signal ready" aria-hidden="true"></span>
					<div>
						<p class="status-title">Version {$availableUpdate?.version} is ready to install.</p>
						<p class="status-sub">EntropiaOrme will close and reopen on the new version.</p>
					</div>
				</div>
				<div class="actions">
					<Button variant="primary" onclick={installUpdate}>Install and restart</Button>
				</div>
			{:else if $updatePhase === 'installing'}
				<div class="status-line">
					<span class="spinner" aria-hidden="true"></span>
					<span>Installing… the app will restart.</span>
				</div>
			{:else if $updatePhase === 'error'}
				<div class="status-head">
					<span class="signal error" aria-hidden="true"></span>
					<div>
						<p class="status-title">Something went wrong.</p>
						<p class="status-sub">{$updateError ?? 'The update could not be completed.'}</p>
					</div>
				</div>
				<div class="actions">
					<Button variant="secondary" onclick={() => checkForUpdate()}>Try again</Button>
				</div>
			{:else if $updatePhase === 'up-to-date'}
				<div class="status-head">
					<span class="signal ready" aria-hidden="true"></span>
					<p class="status-title">You're on the latest version.</p>
				</div>
				<div class="actions">
					<Button variant="secondary" onclick={() => checkForUpdate()}>Check again</Button>
				</div>
			{:else}
				<div class="status-head">
					<p class="status-title">Check whether a newer version is available.</p>
				</div>
				<div class="actions">
					<Button variant="primary" onclick={() => checkForUpdate()}>Check for updates</Button>
				</div>
			{/if}
		</div>
	</section>

	<section class="panel">
		<div class="toggle-row">
			<div class="toggle-copy">
				<span class="toggle-title">Automatic updates</span>
				<span class="toggle-sub">
					Check for new versions on launch and offer to install them. The check sends no data; you
					confirm every install.
				</span>
			</div>
			<Toggle
				checked={$autoUpdateEnabled}
				onchange={onToggleAutoUpdate}
				label="Automatic updates"
			/>
		</div>
	</section>
</div>

<style>
	.page {
		max-width: 640px;
		margin: 0 auto;
		padding: 2.5rem 2rem 3rem;
		display: grid;
		gap: 1.5rem;
	}
	.head {
		display: grid;
		gap: 0.5rem;
	}
	.eyebrow {
		font-size: 0.6875rem;
		letter-spacing: 0.22em;
		text-transform: uppercase;
		color: var(--color-text-tertiary);
		font-weight: 500;
	}
	.title {
		font-size: 1.5rem;
		line-height: 1.2;
		font-weight: 600;
		letter-spacing: -0.012em;
		color: var(--color-text);
		margin: 0;
	}
	.lede {
		margin: 0.25rem 0 0;
		font-size: 0.9rem;
		line-height: 1.6;
		color: var(--color-text-secondary);
		max-width: 56ch;
	}
	code {
		font-family: var(--font-mono, ui-monospace, 'Cascadia Code', monospace);
		font-size: 0.85em;
		padding: 0.05rem 0.3rem;
		border-radius: var(--radius-sm, 4px);
		background: color-mix(in oklab, var(--color-surface) 60%, transparent);
		border: 1px solid color-mix(in oklab, var(--color-border) 60%, transparent);
		color: var(--color-text);
	}
	.panel {
		border: 1px solid color-mix(in oklab, var(--color-border) 90%, transparent);
		border-radius: var(--radius-lg);
		background: color-mix(in oklab, var(--color-surface) 45%, transparent);
		padding: 1.25rem;
		display: grid;
		gap: 1rem;
	}
	.panel-row {
		display: flex;
		gap: 2.5rem;
	}
	.meta {
		display: grid;
		gap: 0.25rem;
	}
	.meta-label {
		font-size: 0.6875rem;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--color-text-tertiary);
	}
	.meta-value {
		font-size: 0.95rem;
		font-weight: 600;
		color: var(--color-text);
		font-variant-numeric: tabular-nums;
	}
	.meta-value.channel {
		color: var(--color-accent);
	}
	.divider {
		height: 1px;
		background: color-mix(in oklab, var(--color-border) 70%, transparent);
	}
	.status {
		display: grid;
		gap: 0.875rem;
	}
	.status-line {
		display: flex;
		align-items: center;
		gap: 0.625rem;
		font-size: 0.9rem;
		color: var(--color-text-secondary);
	}
	.status-head {
		display: flex;
		align-items: flex-start;
		gap: 0.75rem;
	}
	.status-title {
		margin: 0;
		font-size: 0.95rem;
		font-weight: 600;
		color: var(--color-text);
	}
	.status-sub {
		margin: 0.2rem 0 0;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}
	.signal {
		flex-shrink: 0;
		margin-top: 0.35rem;
		width: 0.5rem;
		height: 0.5rem;
		border-radius: 50%;
		background: var(--color-accent);
		box-shadow: 0 0 8px color-mix(in oklab, var(--color-accent) 70%, transparent);
	}
	.signal.ready {
		background: var(--color-success);
		box-shadow: 0 0 8px color-mix(in oklab, var(--color-success) 70%, transparent);
	}
	.signal.error {
		background: var(--color-error);
		box-shadow: 0 0 8px color-mix(in oklab, var(--color-error) 70%, transparent);
	}
	.notes {
		font-size: 0.8125rem;
		line-height: 1.6;
		color: var(--color-text-secondary);
		white-space: pre-wrap;
		max-height: 11rem;
		overflow-y: auto;
		padding: 0.75rem 0.875rem;
		border-radius: var(--radius-md);
		border: 1px solid color-mix(in oklab, var(--color-border) 70%, transparent);
		background: color-mix(in oklab, var(--color-base) 40%, transparent);
	}
	.actions {
		display: flex;
		gap: 0.5rem;
	}
	.bar {
		position: relative;
		height: 6px;
		border-radius: 999px;
		background: color-mix(in oklab, var(--color-border-bright) 60%, transparent);
		overflow: hidden;
	}
	.bar-fill {
		position: absolute;
		inset: 0 auto 0 0;
		width: 0;
		background: var(--color-accent);
		box-shadow: 0 0 8px color-mix(in oklab, var(--color-accent) 70%, transparent);
		transition: width var(--duration-base) var(--ease-out);
	}
	.bar.indeterminate .bar-fill {
		width: 40%;
		animation: indeterminate 1.2s var(--ease-out) infinite;
	}
	@keyframes indeterminate {
		0% {
			transform: translateX(-110%);
		}
		100% {
			transform: translateX(320%);
		}
	}
	.spinner {
		width: 0.9rem;
		height: 0.9rem;
		border-radius: 50%;
		border: 2px solid color-mix(in oklab, var(--color-accent) 30%, transparent);
		border-top-color: var(--color-accent);
		animation: spin 0.7s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.toggle-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1.25rem;
	}
	.toggle-copy {
		display: grid;
		gap: 0.25rem;
	}
	.toggle-title {
		font-size: 0.9rem;
		font-weight: 600;
		color: var(--color-text);
	}
	.toggle-sub {
		font-size: 0.8125rem;
		line-height: 1.5;
		color: var(--color-text-secondary);
		max-width: 44ch;
	}
</style>
