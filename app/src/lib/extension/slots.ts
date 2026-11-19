/**
 * The component slots shared surfaces render. The default: every slot is
 * empty.
 */
import type { Component } from 'svelte';

/** Actions beside the dashboard's header. */
export const headerActions: readonly Component[] = [];

/** Sections at the foot of Settings, in order. */
export const settingsSections: readonly Component[] = [];

/** Marks at the end of the overlay readout's status line. */
export const overlayReadoutMarks: readonly Component[] = [];

/** Banners under the main window's title bar. */
export const shellBanners: readonly Component[] = [];

/** A dashboard widget tab: placed after the shared widgets, before the
 * stats customiser. */
export interface DashboardTab {
	readonly id: string;
	readonly label: string;
	readonly component: Component;
}

export const dashboardTabs: readonly DashboardTab[] = [];

/** Rows at the head of the dashboard's Recent Events tab, kept up while
 * another tab is open so the tab can show that some are unread. */
export interface DashboardEvents<M = unknown> {
	readonly component: Component<{ model: M }>;
	readonly model: M;
	/** Whether it holds rows of its own. */
	readonly hasRows: boolean;
	/** Whether any row is unread. */
	readonly unread: boolean;
	/** The class the tab strip carries while one is. */
	readonly unreadClass: string;
	/** Begin its upkeep; returns the teardown. */
	start(): () => void;
}

/** The events tab's extra rows. The default: none. */
export function createDashboardEvents(): DashboardEvents | null {
	return null;
}
