# ADR-0041: No connection to the project's site; the legal text ships with the source

- Status: Accepted
- Context: amends [ADR-0014](0014-release-engineering.md), completing the removal [ADR-0040](0040-periodic-source-snapshots.md) left to a later snapshot; amends [ADR-0006](0006-tauri-svelte-frontend.md), [ADR-0022](0022-runes-native-shared-state.md) and [ADR-0025](0025-central-market-data-service.md) where they describe the origin, stores and gateway this removes.

## Context and problem statement

ADR-0040 made this repository source-only: no installers, releases or update manifests are published, and building from source is the only way to run the application. It kept the in-application updater in the code and noted that its removal would follow in a later snapshot.

The updater was one of two features that reached the project's own site. The other was the news feed, which fetched its posts from the same origin. With nothing published there for either, both were dead weight, and the news feed was the only reason the webview's content-security policy allowed an external origin at all, through an outbound gateway (`app/src/lib/outboundHttp.ts`) kept for it. The first-run flow carried a networking step in which the player kept or turned off those two connections, and a player who had onboarded before a networked feature arrived was taken back to that step.

The Terms of Service and the Privacy Policy lived on the same site. The first-run flow linked out to them, so the text a player accepted was neither part of the source nor readable without a connection.

## Decision

- **The updater is removed**: its plugins, configuration and signing key, its commands, the update notice and the Updates page.
- **The news feed is removed**: its page, its entry in the navigation rail, its fetch, and the outbound gateway it fetched through.
- **The application's webview may reach no external origin.** The content-security policy's `connect-src` is `'self'` alone, held by a test in the shell crate (`app/src-tauri/entropia-orme/src/lib.rs`). The application makes no connection to the project's site.
- **The networking step leaves the first-run flow**, together with the return to it for players who onboarded earlier: there is no optional connection left to choose.
- **The legal text ships with the source.** `legal/terms.md` and `legal/privacy.md` are the one copy of the Terms of Service and the Privacy Policy. The application imports them at build time and renders them as they stand in the first-run flow (`app/src/lib/legal.ts`), and a test holds each document's version line to the version a player is asked to accept.

## Consequences

- The update path in ADR-0014 (the per-channel manifest, the signed download-then-deferred-install flow, the update notice, and the opt-out networking posture with its onboarding panel) no longer exists. The installer scripts it names remain in the tree for building from source, and nothing publishes their output.
- ADR-0006's content-security policy no longer lists the project's site; the news and updater state named among ADR-0022's frozen modules no longer exists; and ADR-0025's client posture describes a client the application no longer has, since the market layer now reads only what the player enters (see the [market data chapter](../architecture/market-data.md)). Those records are not rewritten; this one amends them.
- What remains of the application's outbound traffic is the account connection: the backend's reader of the player's own account site, which reads only once the player has connected it, and the separate sign-in window in which the player signs in on that site (`app/src-tauri/entropia-orme/src/account_window.rs`).
- A change to the legal text is a change to the source, reviewed like any other, and the version a player accepts is the version printed at the top of the documents they read.

See [ADR-0040](0040-periodic-source-snapshots.md) for the source-only posture this completes, and the [ADR index](index.md).
