/**
 * The shell's start hook, run in every window. The default: nothing to
 * start.
 */

/** Start what the extension needs in every window; returns the teardown. */
export function startShellExtension(): () => void {
	return () => {};
}
