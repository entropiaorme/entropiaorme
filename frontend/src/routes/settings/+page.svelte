<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { Button, Divider, Toggle, Input, SegmentedControl } from '$lib/components';
	import { theme, setTheme, type Theme } from '$lib/theme';
	import { newsOptIn, setNewsOptIn } from '$lib/news';
	import { getSettings, updateSettings } from '$lib/api';
	import type { AppSettings } from '$lib/types';
	import { externalLinks } from '$lib/utils/openExternal';

	let settings = $state<AppSettings | null>(null);
	let loading = $state(true);
	let loadError: string | null = $state(null);

	let savedIndicator: string | null = $state(null);

	// Capability toggles
	let savingField = $state<string | null>(null);
	let capabilityError: string | null = $state(null);

	// Loot filter
	let newFilterItem = $state('');

	// Appearance
	const themeOptions: { id: Theme; label: string }[] = [
		{ id: 'dark', label: 'Dark' },
		{ id: 'light', label: 'Light' }
	];
	async function handleThemeChange(id: string) {
		await setTheme(id as Theme);
	}

	async function refreshSettingsState() {
		settings = await getSettings();
		return settings;
	}

	async function scrollHashTargetIntoView() {
		if (globalThis.location?.hash !== '#cost-attribution') return;
		await tick();
		globalThis.document
			.getElementById('cost-attribution')
			?.scrollIntoView({ block: 'center' });
	}

	onMount(() => {
		const handleHashChange = () => {
			scrollHashTargetIntoView();
		};
		globalThis.addEventListener('hashchange', handleHashChange);

		(async () => {
			try {
				await refreshSettingsState();
			} catch (e) {
				loadError = e instanceof Error ? e.message : 'Failed to load settings';
			} finally {
				loading = false;
				await scrollHashTargetIntoView();
			}
		})();

		return () => {
			globalThis.removeEventListener('hashchange', handleHashChange);
		};
	});

	function flashSaved(field: string) {
		savedIndicator = field;
		setTimeout(() => {
			if (savedIndicator === field) savedIndicator = null;
		}, 1200);
	}

	let chatlogSaveStatus: 'idle' | 'saved' | 'invalid' = $state('idle');

	async function handleChatlogPathChange() {
		if (!settings) return;
		const path = settings.gameConnection.chatLogPath.trim();
		const updated = await updateSettings({ chatlog_path: path });
		settings.gameConnection.chatLogPath = updated.gameConnection.chatLogPath;
		settings.gameConnection.chatLogValid = updated.gameConnection.chatLogValid;
		chatlogSaveStatus = updated.gameConnection.chatLogValid ? 'saved' : 'invalid';
		setTimeout(() => { chatlogSaveStatus = 'idle'; }, 3000);
	}

	async function handlePlayerNameInput() {
		if (!settings) return;
		await updateSettings({ player_name: settings.gameConnection.playerName });
		flashSaved('playerName');
	}

	async function handleHotbarHooks(checked: boolean) {
		if (!settings) return;
		const trifectaReady = settings.trifecta.ready;
		if (!checked && !trifectaReady) {
			capabilityError =
				settings.trifecta.message ??
				'Configure the trifecta in Equipment before disabling the hotbar key listener.';
			return;
		}
		savingField = 'hotbarHooks';
		capabilityError = null;
		try {
			settings = await updateSettings({ hotbar_hooks_enabled: checked });
			flashSaved('hotbarHooks');
		} catch (e) {
			capabilityError = e instanceof Error ? e.message : 'Failed to update hotbar key listener';
		} finally {
			savingField = null;
		}
	}

	async function handleRepairOcr(checked: boolean) {
		if (!settings) return;
		savingField = 'repairOcr';
		capabilityError = null;
		try {
			settings = await updateSettings({ repair_ocr_enabled: checked });
			flashSaved('repairOcr');
		} catch (e) {
			capabilityError = e instanceof Error ? e.message : 'Failed to update repair OCR';
		} finally {
			savingField = null;
		}
	}

	async function handleArmourReminder(checked: boolean) {
		if (!settings) return;
		savingField = 'armourReminder';
		capabilityError = null;
		try {
			settings = await updateSettings({ end_of_session_armour_reminder_enabled: checked });
			flashSaved('armourReminder');
		} catch (e) {
			capabilityError = e instanceof Error ? e.message : 'Failed to update armour reminder';
		} finally {
			savingField = null;
		}
	}

	async function addFilterItem() {
		if (!settings || !newFilterItem.trim()) return;
		const name = newFilterItem.trim();
		if (settings.lootFilterBlacklist.some((n) => n.toLowerCase() === name.toLowerCase())) {
			newFilterItem = '';
			return;
		}
		settings.lootFilterBlacklist = [...settings.lootFilterBlacklist, name];
		newFilterItem = '';
		await updateSettings({ loot_filter_blacklist: settings.lootFilterBlacklist });
		flashSaved('lootFilter');
	}

	async function removeFilterItem(index: number) {
		if (!settings) return;
		settings.lootFilterBlacklist = settings.lootFilterBlacklist.filter((_, i) => i !== index);
		await updateSettings({ loot_filter_blacklist: settings.lootFilterBlacklist });
		flashSaved('lootFilter');
	}

</script>

{#if loading}
	<div class="px-6 pb-6 text-sm text-text-tertiary">Loading settings…</div>
{:else if loadError}
	<div class="px-6 pb-6">
		<div class="rounded-md border border-error/30 bg-error/5 px-3 py-2">
			<p class="text-sm text-error">{loadError}</p>
			<p class="text-xs text-text-tertiary mt-1">Is the backend running?</p>
		</div>
	</div>
{:else if settings}
	<div class="px-6 pb-6 space-y-10 max-w-2xl">
	<!-- Page header -->
	<header class="flex flex-col gap-1.5">
		<h1 class="text-xl font-semibold text-text tracking-tight">Settings</h1>
		<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
		<p class="text-sm text-text-secondary mt-0.5">Connection, tracking, and preferences</p>
	</header>

	{#if capabilityError}
		<div class="rounded-md border border-error/30 bg-error/5 px-3 py-2">
			<p class="text-sm text-error">{capabilityError}</p>
		</div>
	{/if}

	<!-- Cluster: Game connection -->
	<section>
		<h2 class="text-[11px] font-medium uppercase tracking-[0.12em] text-text-tertiary">
			Game connection
		</h2>

		<div class="mt-3">
			<!-- Chat.log path -->
			<div class="py-5 space-y-1.5">
				<label for="chatlog-path" class="text-xs font-medium text-text-secondary">
					Chat.log path
				</label>
				<Input
					id="chatlog-path"
					type="text"
					bind:value={settings.gameConnection.chatLogPath}
					onchange={handleChatlogPathChange}
					placeholder="/path/to/Entropia Universe/chat.log"
				/>
				{#if chatlogSaveStatus === 'saved'}
					<p class="text-xs text-success">Chat.log found, connected.</p>
				{:else if chatlogSaveStatus === 'invalid'}
					<p class="text-xs text-warning">
						Saved, but chat.log not found at this path. Check the file exists.
					</p>
				{:else if !settings.gameConnection.chatLogValid}
					<p class="text-xs text-warning">Chat.log not found at this path.</p>
				{/if}
			</div>

			<Divider />

			<!-- Player avatar name -->
			<div class="py-5 space-y-1.5">
				<label for="player-name" class="text-xs font-medium text-text-secondary">
					Player avatar name
				</label>
				<Input
					id="player-name"
					type="text"
					bind:value={settings.gameConnection.playerName}
					onchange={handlePlayerNameInput}
					placeholder="Firstname Nickname Lastname"
				/>
				<p class="text-xs text-text-tertiary">Used to link Globals.</p>
			</div>
		</div>
	</section>

	<!-- Cluster: Session tracking -->
	<section>
		<h2 class="text-[11px] font-medium uppercase tracking-[0.12em] text-text-tertiary">
			Session tracking
		</h2>

		<div class="mt-3">
			<!-- Hotbar key listener -->
			<div class="py-5 space-y-2">
				<div id="cost-attribution" class="flex items-start justify-between gap-6">
					<div>
						<p class="text-sm text-text">Hotbar key listener</p>
						<p class="text-xs text-text-tertiary mt-0.5">
							Use your number hotbar (1-9, 0) to switch cost tracking of weapons.
							When off, trifecta mode is used (preset small weapon, big weapon, and healing item).
						</p>
					</div>
					<Toggle
						checked={settings.hotbarHooksEnabled}
						disabled={savingField !== null}
						onchange={handleHotbarHooks}
						label="Enable hotbar key listener"
						title={!settings.trifecta.ready && settings.hotbarHooksEnabled
							? settings.trifecta.message ??
								'Configure the trifecta in Equipment before disabling the hotbar key listener'
							: undefined}
					/>
				</div>
				{#if !settings.trifecta.ready}
					<p class="text-xs text-text-tertiary">
						Trifecta:
						{settings.trifecta.message ??
							'set a small weapon, big weapon, and healing tool in Equipment → Trifecta to enable trifecta attribution.'}
					</p>
				{/if}
				{#if savedIndicator === 'hotbarHooks'}
					<p class="text-xs text-success">Saved</p>
				{/if}
			</div>

			<Divider />

			<!-- Repair-cost OCR -->
			<div class="py-5 flex items-start justify-between gap-6">
				<div>
					<p class="text-sm text-text">Repair-cost OCR</p>
					<p class="text-xs text-text-tertiary mt-0.5">
						For UL armour repair cost tracking. Dock the in-game repair terminal at the
						bottom-right of the Entropia Universe window; the cost number is read from a
						fixed region relative to that corner. Manual entry is always available from
						the overlay.
					</p>
					{#if savedIndicator === 'repairOcr'}
						<p class="text-xs text-success mt-1">Saved</p>
					{/if}
				</div>
				<Toggle
					checked={settings.repairOcrEnabled}
					disabled={savingField !== null}
					onchange={handleRepairOcr}
					label="Enable repair-cost OCR"
				/>
			</div>

			<Divider />

			<!-- End-of-session armour reminder -->
			<div class="py-5 flex items-start justify-between gap-6">
				<div>
					<p class="text-sm text-text">End-of-session armour reminder</p>
					<p class="text-xs text-text-tertiary mt-0.5">
						When you stop a session, the Stop button becomes a yellow "Track armour?"
						prompt. Yes opens the armour-cost popup; No finishes the session without it.
						Turn off to stop sessions in one click.
					</p>
					{#if savedIndicator === 'armourReminder'}
						<p class="text-xs text-success mt-1">Saved</p>
					{/if}
				</div>
				<Toggle
					checked={settings.endOfSessionArmourReminderEnabled}
					disabled={savingField !== null}
					onchange={handleArmourReminder}
					label="Enable end-of-session armour reminder"
				/>
			</div>

			<Divider />

			<!-- Loot filter -->
			<div class="py-5 space-y-3">
				<div>
					<p class="text-sm text-text">Loot filter</p>
					<p class="text-xs text-text-tertiary mt-0.5">
						Items matching these names are excluded from tracking returns. Case-insensitive.
					</p>
				</div>

				<div class="flex flex-wrap gap-2">
					{#each settings.lootFilterBlacklist as item, i}
						{@const isDefault = item.toLowerCase() === 'universal ammo'}
						<span
							class="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs rounded-sm
								{isDefault
									? 'bg-surface-hover/50 text-text-tertiary'
									: 'bg-surface-hover text-text-secondary'}"
						>
							{item}
							{#if isDefault}
								<span class="text-[10px] text-text-tertiary/60">(default)</span>
							{:else}
								<button
									type="button"
									class="icon-button-row p-0.5 rounded-sm"
									onclick={() => removeFilterItem(i)}
									aria-label="Remove {item} from filter"
									title="Remove"
								>
									<svg
										xmlns="http://www.w3.org/2000/svg"
										viewBox="0 0 16 16"
										fill="currentColor"
										class="w-3 h-3"
									>
										<path
											d="M5.28 4.22a.75.75 0 0 0-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 1 0 1.06 1.06L8 9.06l2.72 2.72a.75.75 0 1 0 1.06-1.06L9.06 8l2.72-2.72a.75.75 0 0 0-1.06-1.06L8 6.94 5.28 4.22Z"
										/>
									</svg>
								</button>
							{/if}
						</span>
					{/each}
				</div>

				<div class="flex items-center gap-2">
					<Input
						type="text"
						bind:value={newFilterItem}
						placeholder="Item name..."
						class="flex-1"
						onkeydown={(e) => { if (e.key === 'Enter') addFilterItem(); }}
					/>
					<Button variant="secondary" size="sm" onclick={addFilterItem} disabled={!newFilterItem.trim()}>
						Add
					</Button>
					{#if savedIndicator === 'lootFilter'}
						<span class="text-xs text-success transition-opacity duration-[var(--duration-base)]">
							Saved
						</span>
					{/if}
				</div>
			</div>
		</div>
	</section>

	<!-- Cluster: Preferences -->
	<section>
		<h2 class="text-[11px] font-medium uppercase tracking-[0.12em] text-text-tertiary">
			Preferences
		</h2>

		<div class="mt-3">
			<!-- Theme -->
			<div class="py-5 flex items-start justify-between gap-6">
				<div>
					<p class="text-sm text-text">Theme</p>
					<p class="text-xs text-text-tertiary mt-0.5">
						Switches the app between dark and light rendering.
					</p>
				</div>
				<SegmentedControl
					options={themeOptions}
					active={$theme}
					onchange={handleThemeChange}
					size="md"
				/>
			</div>

			<Divider />

			<!-- News & Updates -->
			<div class="py-5 flex items-start justify-between gap-6">
				<div>
					<p class="text-sm text-text">News &amp; Updates</p>
					<p class="text-xs text-text-tertiary mt-0.5">
						Off by default. When enabled, the app fetches a small list of articles and release
						notices from the project website (<code>entropiaorme.com</code>). Download-only. No
						background polling, no telemetry.
					</p>
				</div>
				<Toggle
					checked={$newsOptIn}
					onchange={setNewsOptIn}
					label="Enable News &amp; Updates"
				/>
			</div>
		</div>
	</section>

	<!-- Database path: metadata, not a knob -->
	<div class="flex items-baseline gap-3 text-xs text-text-tertiary pt-2">
		<span class="font-medium text-text-secondary shrink-0">Database</span>
		<span class="tabular-nums truncate" title={settings.dbPath}>{settings.dbPath}</span>
	</div>

	<!-- Footer: brand + attribution -->
	<footer class="pt-10 pb-2 flex flex-col items-center gap-3.5">
		<div class="flex items-center gap-3">
			<img
				src={$theme === 'light' ? '/wordmark-on-light.png' : '/wordmark-on-dark.png'}
				alt="EntropiaOrme"
				class="h-[1.875rem] w-auto opacity-60 select-none"
				draggable="false"
			/>
			<span class="text-[11px] text-text-tertiary tabular-nums">v{settings.appVersion}</span>
		</div>
		<a
			href="https://entropianexus.com/"
			target="_blank"
			rel="noopener noreferrer"
			class="inline-flex items-center gap-1 text-[11px] text-text-tertiary hover:text-accent transition-colors duration-[var(--duration-base)] ease-[var(--ease-out)]"
			use:externalLinks
		>
			<span>Game data from Entropia&nbsp;Nexus</span>
			<svg viewBox="0 0 20 20" fill="currentColor" class="w-2.5 h-2.5" aria-hidden="true">
				<path
					fill-rule="evenodd"
					d="M5.22 14.78a.75.75 0 001.06 0l7.22-7.22v5.69a.75.75 0 001.5 0v-7.5a.75.75 0 00-.75-.75h-7.5a.75.75 0 000 1.5h5.69l-7.22 7.22a.75.75 0 000 1.06z"
					clip-rule="evenodd"
				/>
			</svg>
		</a>
	</footer>
</div>
{/if}
