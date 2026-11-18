/**
 * The Tauri-bus topic the shell's event bridge emits each backend tracking
 * frame on: the colon form of the `tracking.session.updated` wire topic (Tauri
 * event names forbid dots). See `spawn_domain_event_bridge` in the shell.
 *
 * Kept apart from the snapshot store so a surface that only listens for the
 * topic does not load the store.
 */
export const TRACKING_TOPIC = 'tracking:session:updated';
