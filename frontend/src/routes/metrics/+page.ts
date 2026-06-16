// The hidden developer metrics page reads live in-process telemetry at runtime
// and is gated behind developer mode (the backend 404s otherwise), so it is
// never prerendered: it is a client-only developer surface, deliberately absent
// from the navigation.
export const prerender = false;
