# ADR-0031: Protection costs recorded at session grain, when the player repairs

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

Armour and plates decay with the damage they absorb, which the chat log does not report. The first protection model therefore tried to know which protection absorbed each hit. The player named loadouts in Equipment, declared the active one from the overlay during play, and was asked at the end of a session which setup had been worn. Every defensive hit was stamped with the declared loadout, and a later repair or Trade Terminal reading settled only the hits of the matching layer.

Living with that model showed that its cost fell on the player, not the code. A forgotten switch silently mis-attributed every hit after it. A deferred recording could leave a session stranded until its setup was named. The end-of-session question arrived at exactly the moment a player wants to leave. And the precision it bought, protection cost per segment within a session, rested on the declaration being right.

## Decision

**Nothing about protection is declared during play.** The tracker records every observed defensive hit (numeric damage or a deflection) with its session and event context, as before, and nothing more. There are no loadouts, no live selection, no protection intervals, and no stop prompt.

**Costs are recorded per stream, when the player repairs or scans.** Unlimited protection is one pooled stream: a confirmed Repair Terminal total is consumed at raw TT, and which piece or plate it came from is not asked. Each limited armour or plate set is its own stream, because its acquisition markup is its own. Its consumption is the TT lost between two Trade Terminal readings at the set's frozen markup, and its first reading is a baseline. Recording happens in the overlay's Cost popup and needs no running session.

**A recording looks back to the previous recording of its stream and is spread over the sessions the player ticks.** Each stream keeps a position on the defence-event stream (`protection_cost_windows.evidence_cursor`, or a limited set's baseline observation cursor). The popup offers the sessions with hits since that position, grouped by session type and all ticked. The player unticks a type, or a single session within one, that did not use that protection. The look-back can be started later. For the unlimited pool, sessions an earlier repair already covered can be re-included, for a piece left unrepaired last time; they are weighed by all of their hits. A limited reading cannot reach back past its own baseline.

**Allocation is by equal observed hits, twice.** A recording's cost is split across its ticked sessions by hit count, and within each session across its contexts by hit count. Streams that cover the same session add up. The last context takes the rounding residual, so the stored allocations sum to the recorded cost exactly. A recording with no ticked session is kept as an explicit amount no session carries.

**A session no recording reaches reads as not recorded.** Its armour cost is shown as not recorded yet, never as a zero it has not earned, until any recording covers it.

## Alternatives considered

- **Keep per-hit layer identity and make the declaration easier.** Rejected: any in-play declaration can be forgotten, and a forgotten one corrupts data silently. The approximation it protected, protection cost per segment, is worth less than the account-level truth it put at risk.
- **Spread every recording over every session since the last one, with no choice.** Rejected: a player who alternates limited and unlimited protection across sessions would have a limited set's markup-priced loss charged to sessions that never wore it. The tick list is the one piece of input the model cannot infer, and it is asked for at the moment the player is already recording.
- **Infer which protection was worn from damage magnitudes.** Rejected: incoming damage does not reveal the protection that absorbed it, which is the reason equal-hit weighting was adopted in the first place.

## Consequences

- Session and account protection totals are exact. A segment's share is a hit-weighted estimate, which is the accepted trade for never asking during play.
- Sessions are knowingly imprecise between playing and recording, and say so.
- Loadouts, protection intervals, the per-segment session flag, and the end-of-session reminder setting fall out of use. The migration archives the unlimited sets and loadouts, which releases their names for new limited sets; their rows, like the retired tables and columns, stay readable history. Nothing is dropped or rewritten.
- The pooled unlimited stream cannot tell apart two unlimited items repaired on different days. The re-include option exists for that case, and the account total is right either way.

See also the [database schema](../architecture/database-schema.md), the [service map](../architecture/service-map.md), and the [ADR index](index.md).
