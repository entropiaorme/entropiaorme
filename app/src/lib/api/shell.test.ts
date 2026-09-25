import { beforeEach, describe, expect, it, vi } from 'vitest';

// The shell surface is the single home for bare-command invokes; these
// tests pin each wrapper's command name and argument shape. `invoke` is
// captured at module load, so the mock is hoisted and the module is
// re-imported per test.
const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

type Mod = typeof import('./shell');

async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./shell');
}

beforeEach(() => {
	invokeMock.mockReset();
	invokeMock.mockResolvedValue(undefined);
});

describe('window commands', () => {
	it.each([
		['toggleOverlay', 'toggle_overlay'],
		['toggleCartographyOverlay', 'toggle_cartography_overlay'],
		['showNavigationOverlays', 'show_navigation_overlays'],
		['hideNavigationOverlays', 'hide_navigation_overlays'],
		['showScanOverlay', 'show_scan_overlay'],
		['hideScanOverlay', 'hide_scan_overlay'],
		['showSaleCaptureOverlay', 'show_sale_capture_overlay'],
		['captureSaleFromOverlay', 'capture_sale_from_overlay'],
		['hideSaleCaptureOverlay', 'hide_sale_capture_overlay'],
	] as const)('%s invokes %s with no arguments', async (fn, command) => {
		const shell = await loadModule();
		await shell[fn]();
		expect(invokeMock).toHaveBeenCalledWith(command);
	});

	it('hands route-area selection to the main Maps window with typed context', async () => {
		const { beginNavigationAreaSelection } = await loadModule();
		await beginNavigationAreaSelection(4, 'Calypso', 9);
		expect(invokeMock).toHaveBeenCalledWith('begin_navigation_area_selection', {
			request_id: 4,
			planet: 'Calypso',
			map_view_id: 9,
		});
	});
});

describe('updater commands', () => {
	it.each([
		['checkForUpdate', 'check_for_update'],
		['downloadUpdate', 'download_update'],
		['getUpdateChannel', 'get_update_channel'],
		['installUpdate', 'install_update'],
	] as const)('%s invokes %s', async (fn, command) => {
		const shell = await loadModule();
		await shell[fn]();
		expect(invokeMock).toHaveBeenCalledWith(command);
	});
});

describe('planetMapImage', () => {
	it('fetches the raster over the planet_map_image command as a base64 data URL', async () => {
		const { planetMapImage } = await loadModule();
		invokeMock.mockResolvedValue('aGVsbG8=');
		const url = await planetMapImage('Calypso', 'image/jpeg');
		expect(invokeMock).toHaveBeenCalledWith('planet_map_image', { planet: 'Calypso' });
		expect(url).toBe('data:image/jpeg;base64,aGVsbG8=');
	});
});

describe('manualSkillScanCapturePng', () => {
	it('fetches the capture preview over the capture_png command as a base64 data URL', async () => {
		const { manualSkillScanCapturePng } = await loadModule();
		invokeMock.mockResolvedValue('aGVsbG8=');
		const url = await manualSkillScanCapturePng(3);
		expect(invokeMock).toHaveBeenCalledWith('capture_png', { page: 3 });
		expect(url).toBe('data:image/png;base64,aGVsbG8=');
	});
});

describe('startup readiness', () => {
	it('awaits the settled outcome through substrate_ready', async () => {
		const { awaitSubstrate } = await loadModule();
		invokeMock.mockResolvedValue({ state: 'ready' });
		await expect(awaitSubstrate()).resolves.toEqual({ state: 'ready' });
		expect(invokeMock).toHaveBeenCalledWith('substrate_ready');
	});

	it('relaunches through restart_app', async () => {
		const { restartApp } = await loadModule();
		await restartApp();
		expect(invokeMock).toHaveBeenCalledWith('restart_app');
	});
});
