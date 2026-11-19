<script lang="ts">
	import '../app.css';
	import favicon from '$lib/assets/favicon.png';
	import { Sidebar, Titlebar } from '$lib/components';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { getOnboardingComplete } from '$lib/onboarding';
	import { isTosAccepted } from '$lib/tos';
	import { theme, initTheme } from '$lib/theme.svelte';
	import { initStatsCustomisation } from '$lib/statsCustomisation.svelte';
	import { initStatsScope, statsScope, STATS_SCOPE_CHANGED_EVENT, type StatsScope } from '$lib/statsScope.svelte';
	import { listen } from '@tauri-apps/api/event';
	import { initLooterBasis } from '$lib/looterBasis.svelte';
	import { followLiveMarkupSettings, initLiveMarkupSettings } from '$lib/liveMarkupSettings.svelte';
	import { purgeRetiredPreferences } from '$lib/preferences';
	import { routeRefusal } from '$lib/extension/gate';
	import { startShellExtension } from '$lib/extension/shell';
	import { accountOffered } from '$lib/features/account/offered';
	import { shellBanners } from '$lib/extension/slots';
	import { substrate, whenSubstrateReady } from '$lib/api/readiness.svelte';
	import StartupFailure from '$lib/features/startup/StartupFailure.svelte';
	import StartupIndicator from '$lib/features/startup/StartupIndicator.svelte';
	import { activityMarks } from '$lib/features/account/activityMarks.svelte';
	import {
		NavDashboard,
		NavAnalytics,
		NavCharacter,
		NavQuests,
		NavEquipment,
		NavInventory,
		NavSettings,
	} from '$lib/icons';

	let { children } = $props();

	// Every window runs the extension's start hook.
	onMount(() => startShellExtension());

	// Ask whether the backend has started as soon as this window exists, not
	// when its first page happens to read something: the startup indicator
	// and the failure surface follow this answer. Pages mount straight away
	// either way; the typed transport holds their reads until it is ready.
	// A failed start is shown by the failure surface, not thrown here.
	void whenSubstrateReady().catch(() => {});

	// The stats scope is shared with the overlay window, so a flip made
	// there moves this window's figures too.
	onMount(() => {
		let unlisten: (() => void) | undefined;
		let unmounted = false;
		void (async () => {
			const detach = await listen<StatsScope>(STATS_SCOPE_CHANGED_EVENT, (event) => {
				if (event.payload === 'instance' || event.payload === 'lifetime') {
					statsScope.current = event.payload;
				}
			});
			// Guard the unmount-before-resolve race: if teardown already ran,
			// detach immediately rather than leaking the listener.
			if (unmounted) detach();
			else unlisten = detach;
		})();
		// So are the live markup settings (confidence and looter basis).
		let unlistenLive: (() => void) | undefined;
		void followLiveMarkupSettings().then((detach) => {
			if (unmounted) detach();
			else unlistenLive = detach;
		});
		return () => {
			unmounted = true;
			unlisten?.();
			unlistenLive?.();
		};
	});

	let onboardingChecked = $state(false);
	let welcomingIn = $state(false);

	$effect(() => {
		if (typeof document !== 'undefined') {
			document.documentElement.setAttribute('data-theme', theme.current);
		}
	});

	onMount(async () => {
		await Promise.all([
			initTheme(),
			initStatsCustomisation(),
			initStatsScope(),
			purgeRetiredPreferences(),
			initLooterBasis(),
			initLiveMarkupSettings(),
		]);
		if (
			typeof sessionStorage !== 'undefined' &&
			sessionStorage.getItem('welcome_just_finished') === '1'
		) {
			sessionStorage.removeItem('welcome_just_finished');
			welcomingIn = true;
		}
		const complete = await getOnboardingComplete();
		const path = page.url.pathname;
		const isWelcome = path.startsWith('/welcome');
		const isOverlay =
			path.startsWith('/overlay') ||
			path.startsWith('/scan-overlay') ||
			path.startsWith('/cartography-overlay') ||
			path.startsWith('/navigation-hud') ||
			path.startsWith('/radar-guidance');
		if (!isWelcome && !isOverlay) {
			if (!complete) {
				await goto('/welcome', { replaceState: true });
			} else if (!(await isTosAccepted())) {
				await goto('/welcome/terms', { replaceState: true });
			}
		}
		onboardingChecked = true;
	});

	// Something new from the account marks the Inventory until the player
	// opens what the account did (see `activityMarks`).
	const baseNavItems = $derived([
		{ id: '/', label: 'Dashboard', icon: NavDashboard },
		{ id: '/analytics', label: 'Analytics', icon: NavAnalytics },
		{
			id: '/inventory',
			label: 'Inventory',
			icon: NavInventory,
			...(activityMarks.anyNew ? { indicator: 'attention' as const } : {}),
		},
		{ id: '/character', label: 'Character', icon: NavCharacter },
		{ id: '/quests', label: 'Quests', icon: NavQuests },
		{ id: '/equipment', label: 'Equipment', icon: NavEquipment },
	]);
	// An entry whose route the gate refuses stays in the rail, greyed, with
	// the reason as its note.
	let navItems = $derived(
		baseNavItems.map((item) => {
			const refusal = routeRefusal(item.id);
			return refusal === null
				? item
				: { ...item, disabled: true, note: `${item.label}: ${refusal}` };
		}),
	);

	// The marks are read only where the sidebar shows: the main window,
	// once past onboarding, and only with the other account surfaces. Every
	// other window is refused the account's commands.
	const showsShell = $derived(
		onboardingChecked &&
			!page.url.pathname.startsWith('/welcome') &&
			!page.url.pathname.startsWith('/overlay') &&
			!page.url.pathname.startsWith('/scan-overlay') &&
			!page.url.pathname.startsWith('/cartography-overlay') &&
			!page.url.pathname.startsWith('/navigation-hud') &&
			!page.url.pathname.startsWith('/radar-guidance'),
	);
	$effect(() => {
		if (!showsShell || !accountOffered()) return;
		return activityMarks.connect();
	});

	const settingsItem = { id: '/settings', label: 'Settings', icon: NavSettings };

	// Determine active page from current route
	let activePage = $derived(
		page.url.pathname === '/' ? '/' : '/' + page.url.pathname.split('/')[1]
	);

	function handleNavigate(id: string) {
		if (id !== page.url.pathname) {
			goto(id);
		}
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

{#if page.url.pathname.startsWith('/overlay') || page.url.pathname.startsWith('/scan-overlay') || page.url.pathname.startsWith('/cartography-overlay') || page.url.pathname.startsWith('/navigation-hud') || page.url.pathname.startsWith('/radar-guidance')}
	{@render children()}
{:else if page.url.pathname.startsWith('/welcome')}
	<div class="flex flex-col h-screen bg-base">
		<Titlebar />
		<div class="flex-1 overflow-hidden">
			{@render children()}
		</div>
	</div>
{:else if onboardingChecked}
	<div class="flex h-screen bg-base overflow-hidden" class:welcoming-in={welcomingIn}>
		<Sidebar
			items={navItems}
			active={activePage}
			onnavigate={handleNavigate}
			{settingsItem}
			disabled={substrate.phase === 'failed'}
		/>
		<div class="relative flex flex-col flex-1 overflow-hidden">
			<Titlebar />
			{#each shellBanners as Banner}<Banner />{/each}
			{#if substrate.phase === 'starting'}
				<StartupIndicator />
			{/if}
			<main class="flex-1 min-h-0 overflow-y-auto">
				{#if substrate.failure}
					<StartupFailure failure={substrate.failure} />
				{:else}
					{@render children()}
				{/if}
			</main>
		</div>
	</div>
{/if}

<style>
	.welcoming-in {
		animation: welcoming-in 520ms var(--ease-out) both;
	}
	@keyframes welcoming-in {
		from {
			opacity: 0;
			transform: scale(0.985);
		}
		to {
			opacity: 1;
			transform: scale(1);
		}
	}
</style>
