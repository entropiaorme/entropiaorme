<script lang="ts">
	// The dashboard's account control, level with the page title: an offer
	// to sign in while not signed in, a quiet green "Signed in" once
	// signed in, and a pulsing amber "Signed out" with the offer again when
	// the site ends the sign-in. Disconnecting and the explanations stay on
	// the Settings row. Rendered only where the in-development channel
	// shows the account connection.
	import { onMount } from 'svelte';
	import { Button } from '$lib/components';
	import { type SignalMark, signInView } from './accountSignal';
	import { createConnectionModel } from './connectionModel.svelte';

	const model = createConnectionModel();
	onMount(() => model.connect());

	const view = $derived(
		model.status ? signInView(model.status.signal, model.status.signingIn) : null,
	);

	const DOT: Record<SignalMark, string> = {
		live: 'positive',
		alert: 'warning animate-pulse',
		absent: 'idle',
		checking: 'idle',
	};
	const TEXT: Record<SignalMark, string> = {
		live: 'text-success',
		alert: 'text-warning',
		absent: 'text-text-tertiary',
		checking: 'text-text-tertiary',
	};
</script>

{#if view}
	<div class="flex flex-col items-end gap-1" data-testid="account-sign-in">
		<div class="flex items-center gap-3" title={view.hint}>
			{#if view.status}
				<span
					class="flex items-center gap-2 text-sm {TEXT[view.status.mark]}"
					role="status"
					data-mark={view.status.mark}
				>
					<span class="signal-dot {DOT[view.status.mark]} shrink-0" aria-hidden="true"></span>
					{view.status.text}
				</span>
			{/if}
			{#if view.action}
				<Button
					variant={view.status?.mark === 'alert' ? 'primary' : 'secondary'}
					size="sm"
					loading={model.busy === 'signIn'}
					disabled={model.busy !== null}
					onclick={() => model.signIn()}
				>
					{view.action}
				</Button>
			{/if}
		</div>
		{#if model.actionError}
			<p class="text-xs text-error" role="alert">{model.actionError}</p>
		{/if}
	</div>
{/if}
