<script lang="ts">
	// The Settings cluster for the Entropia Universe account: one row whose
	// content follows the connection, and, once the player has connected,
	// one quiet line on what is read and when, and the items the readings
	// could not place, to place once. Rendered inside a block already gated
	// on the in-development channel.
	import { onMount } from 'svelte';
	import { Button, ErrorNotice, Skeleton } from '$lib/components';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { type ConnectionTone, READING_NOTE, SIGN_IN_NOTE } from './connection';
	import { createConnectionModel } from './connectionModel.svelte';
	import CostPlacements from './CostPlacements.svelte';

	const model = createConnectionModel();
	onMount(() => model.connect());

	const DOT: Record<Exclude<ConnectionTone, 'none'>, string> = {
		positive: 'positive',
		warning: 'warning',
		neutral: 'idle',
	};
</script>

<section aria-labelledby="account-heading" data-testid="account-settings">
	<div class="flex flex-wrap items-center gap-2">
		<h2
			id="account-heading"
			class="text-[11px] font-medium uppercase tracking-[0.12em] text-text-tertiary"
		>
			Entropia Universe account
		</h2>
		<InDevelopmentMark id="account-readings" align="left" />
	</div>

	<div class="mt-3">
		<div class="py-5 space-y-2" aria-busy={!model.view && !model.error}>
			{#if model.view}
				{@const view = model.view}
				<div class="flex items-start justify-between gap-6">
					<div class="min-w-0">
						<div class="flex items-center gap-2">
							{#if view.tone !== 'none'}
								<span class="signal-dot {DOT[view.tone]} shrink-0" aria-hidden="true"></span>
							{/if}
							<p class="text-sm text-text">{view.title}</p>
							{#if view.explainsSignIn}
								<InfoTip align="left" label="About signing in">
									<p class="text-xs leading-relaxed text-text-secondary">
										The account site can see that this app reads your account, and when.
									</p>
									<p class="mt-2 text-xs leading-relaxed text-text-secondary">
										Signing in lasts until the app closes or the site ends the session.
									</p>
									<p class="mt-2 text-xs leading-relaxed text-text-secondary">
										Disconnecting forgets the session in the app; it does not sign you out on the
										site.
									</p>
								</InfoTip>
							{/if}
						</div>
						{#if view.explainsSignIn}
							<p class="text-xs text-text-tertiary mt-0.5 max-w-xl">{SIGN_IN_NOTE}</p>
						{/if}
						{#if view.lines.length > 0}
							<p class="text-xs text-text-tertiary mt-0.5">
								{view.lines.join(' · ')}
							</p>
						{/if}
					</div>
					<div class="flex shrink-0 items-center gap-2">
						{#if view.disconnect}
							<Button
								variant="ghost"
								size="sm"
								loading={model.busy === 'disconnect'}
								disabled={model.busy !== null}
								onclick={() => model.disconnect()}
							>
								Disconnect
							</Button>
						{/if}
						{#if view.signIn}
							<Button
								variant="primary"
								size="sm"
								loading={model.busy === 'signIn'}
								disabled={model.busy !== null}
								onclick={() => model.signIn()}
							>
								{view.signIn}
							</Button>
						{/if}
					</div>
				</div>
				{#if view.describesReading}
					<p class="text-xs text-text-tertiary max-w-xl">{READING_NOTE}</p>
				{/if}
				<ErrorNotice message={model.actionError} onDismiss={() => model.dismissActionError()} />
			{:else if model.error}
				<ErrorNotice message={model.error} />
			{:else}
				<Skeleton class="h-4 w-40" />
				<Skeleton class="h-3 w-72" />
			{/if}
		</div>
		<CostPlacements connected={model.view?.describesReading ?? false} />
	</div>
</section>
