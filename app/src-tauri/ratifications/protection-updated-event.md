# Ratification: protection domain event

Adversarial review of the new `protection.updated` domain-event contract added
to the committed event-schema snapshot.

## Findings

The event-schema snapshot adds exactly one closed envelope and its payload.
`ProtectionUpdated` carries the four envelope properties (`type`,
`event_version`, `occurred_at`, `payload`), requires only `occurred_at` and
`payload`, pins `type` to the `protection.updated` literal as both const and
default, and defaults `event_version` to 1. This matches the native struct:
`deny_unknown_fields`, a closed topic tag that accepts only its own literal,
and a serde default on the version. Apart from its name and description it is
structurally identical to the existing `NavigationUpdated` definition.

`ProtectionUpdatedPayload` is a closed empty object with no required members,
matching the native empty struct with `deny_unknown_fields`. It is a
content-free push-to-pull invalidation: consumers re-read the armour costs
they show through their typed read commands.

The discriminator mapping gains the single `protection.updated` key, and
`oneOf` gains a single member appended after the existing four. A structural
comparison of the old and new documents confirms that no existing definition,
mapping entry, `oneOf` member, or top-level key changed. No other golden moved
across the reviewed range.

The pinned output contains no ambient input. The live envelope's `occurred_at`
is stamped from the injected clock at the publish site, and the wire-bytes unit
test fixes its timestamp.

Behaviour behind the pin was read as well as the diff. The service announces a
change only after a successful commit on the record, undo, and set create,
edit, remove, and restore paths. A refused write returns before the
announcement, and restoring a set that is already active returns early. The
review found that an idempotent replay of a confirmation (same client token),
which writes nothing, still announced; that was corrected before landing, and
the service unit tests now pin that refusals and replays announce nothing.

Focused verification passed: the eo-wire suite 66/66, including event-schema
conformance 8/8 and the new protection wire-bytes test.

Nothing in the reviewed golden delta is unaccounted for. This is a genuine,
deterministic, additive contract extension.

```text
ORACLE-RATIFICATION
range: e06359d..HEAD
goldens: app/src-tauri/contracts/event_schemas.snapshot.json
VERDICT: ratification-sound
```
