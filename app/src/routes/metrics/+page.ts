// The metrics page reads live in-process telemetry at runtime, so it is
// never prerendered: it is a client-only surface, reached from Settings.
export const prerender = false;
