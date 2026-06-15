<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { request, ApiError } from '$lib/api/client';

	type Bucket = { bound_us: number | null; count: number };
	type Histogram = { count: number; sum_us: number; buckets: Bucket[] };
	type MetricsSnapshot = {
		events_published: number;
		http_requests: number;
		ocr_latency: Histogram;
		db_query_latency: Histogram;
		http_request_latency: Histogram;
		rss_bytes: number;
		handle_count: number;
	};

	const POLL_INTERVAL_MS = 2000;

	let snapshot = $state<MetricsSnapshot | null>(null);
	let developerModeOff = $state(false);
	let errorMessage = $state<string | null>(null);
	let crashReporting = $state<boolean | null>(null);

	let pollTimer: ReturnType<typeof setInterval> | null = null;

	async function refreshMetrics(): Promise<void> {
		try {
			snapshot = await request<MetricsSnapshot>('/dev/metrics');
			developerModeOff = false;
			errorMessage = null;
		} catch (err) {
			if (err instanceof ApiError && err.status === 404) {
				developerModeOff = true;
				snapshot = null;
			} else {
				errorMessage = err instanceof Error ? err.message : String(err);
			}
		}
	}

	async function refreshCrashReporting(): Promise<void> {
		try {
			const body = await request<{ crash_reporting_enabled: boolean }>('/dev/crash-reporting');
			crashReporting = body.crash_reporting_enabled;
		} catch {
			crashReporting = null;
		}
	}

	async function toggleCrashReporting(enabled: boolean): Promise<void> {
		try {
			const body = await request<{ crash_reporting_enabled: boolean }>('/dev/crash-reporting', {
				method: 'POST',
				body: JSON.stringify({ crash_reporting_enabled: enabled })
			});
			crashReporting = body.crash_reporting_enabled;
		} catch (err) {
			errorMessage = err instanceof Error ? err.message : String(err);
		}
	}

	function formatBytes(bytes: number): string {
		if (bytes <= 0) return '—';
		return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
	}

	function formatGauge(value: number): string {
		return value > 0 ? value.toLocaleString() : '—';
	}

	function meanMs(histogram: Histogram): string {
		if (histogram.count === 0) return '—';
		return `${(histogram.sum_us / histogram.count / 1000).toFixed(2)} ms`;
	}

	function bucketLabel(bound_us: number | null): string {
		if (bound_us === null) return '1s+';
		if (bound_us < 1000) return `${bound_us}µs`;
		return `${bound_us / 1000}ms`;
	}

	function maxBucket(histogram: Histogram): number {
		return histogram.buckets.reduce((max, bucket) => Math.max(max, bucket.count), 0);
	}

	onMount(() => {
		void refreshMetrics();
		void refreshCrashReporting();
		pollTimer = setInterval(() => {
			if (typeof document !== 'undefined' && document.hidden) return;
			void refreshMetrics();
		}, POLL_INTERVAL_MS);
	});

	onDestroy(() => {
		if (pollTimer) clearInterval(pollTimer);
	});
</script>

<svelte:head><title>Developer metrics</title></svelte:head>

{#snippet histogramCard(title: string, histogram: Histogram)}
	<div class="rounded-lg border border-slate-700 bg-slate-800/50 p-4">
		<div class="flex items-baseline justify-between">
			<h3 class="text-sm font-medium text-slate-200">{title}</h3>
			<span class="text-xs text-slate-400">{histogram.count.toLocaleString()} samples</span>
		</div>
		<p class="mt-1 text-2xl font-semibold text-slate-50">{meanMs(histogram)}<span class="ml-1 text-xs font-normal text-slate-400">mean</span></p>
		<div class="mt-3 space-y-1">
			{#each histogram.buckets as bucket (bucket.bound_us ?? 'overflow')}
				{@const peak = maxBucket(histogram)}
				<div class="flex items-center gap-2 text-[11px] text-slate-400">
					<span class="w-12 shrink-0 text-right tabular-nums">{bucketLabel(bucket.bound_us)}</span>
					<div class="h-2 flex-1 overflow-hidden rounded bg-slate-700">
						<div
							class="h-full rounded bg-emerald-500/70"
							style="width: {peak > 0 ? (bucket.count / peak) * 100 : 0}%"
						></div>
					</div>
					<span class="w-10 shrink-0 tabular-nums">{bucket.count}</span>
				</div>
			{/each}
		</div>
	</div>
{/snippet}

<main class="mx-auto max-w-4xl p-6 text-slate-100">
	<header class="mb-6">
		<h1 class="text-xl font-semibold">Developer metrics</h1>
		<p class="text-sm text-slate-400">
			Live in-process telemetry: event throughput, OCR / database / request latencies, and
			resource-drift gauges. Refreshes every {POLL_INTERVAL_MS / 1000}s while this tab is visible.
		</p>
	</header>

	{#if developerModeOff}
		<div class="rounded-lg border border-amber-700/60 bg-amber-900/20 p-4 text-sm text-amber-200">
			Enable <span class="font-medium">Developer mode</span> in Settings to view in-process metrics.
		</div>
	{:else if errorMessage}
		<div class="rounded-lg border border-rose-700/60 bg-rose-900/20 p-4 text-sm text-rose-200">
			Could not read metrics: {errorMessage}
		</div>
	{:else if snapshot}
		<section class="grid grid-cols-2 gap-4 sm:grid-cols-4">
			<div class="rounded-lg border border-slate-700 bg-slate-800/50 p-4">
				<p class="text-xs text-slate-400">Events published</p>
				<p class="mt-1 text-2xl font-semibold">{snapshot.events_published.toLocaleString()}</p>
			</div>
			<div class="rounded-lg border border-slate-700 bg-slate-800/50 p-4">
				<p class="text-xs text-slate-400">HTTP requests</p>
				<p class="mt-1 text-2xl font-semibold">{snapshot.http_requests.toLocaleString()}</p>
			</div>
			<div class="rounded-lg border border-slate-700 bg-slate-800/50 p-4">
				<p class="text-xs text-slate-400">Resident set</p>
				<p class="mt-1 text-2xl font-semibold">{formatBytes(snapshot.rss_bytes)}</p>
			</div>
			<div class="rounded-lg border border-slate-700 bg-slate-800/50 p-4">
				<p class="text-xs text-slate-400">OS handles</p>
				<p class="mt-1 text-2xl font-semibold">{formatGauge(snapshot.handle_count)}</p>
			</div>
		</section>

		<section class="mt-4 grid grid-cols-1 gap-4 sm:grid-cols-3">
			{@render histogramCard('OCR inference latency', snapshot.ocr_latency)}
			{@render histogramCard('Database query latency', snapshot.db_query_latency)}
			{@render histogramCard('HTTP request latency', snapshot.http_request_latency)}
		</section>
	{:else}
		<p class="text-sm text-slate-400">Loading…</p>
	{/if}

	{#if !developerModeOff && crashReporting !== null}
		<section class="mt-6 rounded-lg border border-slate-700 bg-slate-800/50 p-4">
			<label class="flex items-center justify-between gap-4">
				<span>
					<span class="block text-sm font-medium text-slate-200">Crash reporting</span>
					<span class="block text-xs text-slate-400">
						Off by default. When on, a panic writes a PII-scrubbed report locally under the data
						directory. Nothing leaves your machine.
					</span>
				</span>
				<input
					type="checkbox"
					class="h-5 w-5 shrink-0 accent-emerald-500"
					checked={crashReporting}
					onchange={(event) => toggleCrashReporting(event.currentTarget.checked)}
				/>
			</label>
		</section>
	{/if}
</main>
