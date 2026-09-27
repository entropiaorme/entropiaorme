# ADR-0038: Promotion carries the public surface's polish

- Status: Accepted
- Context: amends [ADR-0029](0029-two-line-development.md). EntropiaOrme is a personal application built in public: its maintainer builds it for their own play, and its public audience is developers and players reading the repository for ideas rather than users installing it.

## Context and problem statement

ADR-0029 split development into `next`, where work lands by direct push, and `main`, reached by promotion. It still kept one of the trunk's presentation rules hard on `next`: an unfinished surface had to be marked as in development where it renders, in the same change that introduced it. In practice the same expectation had spread to the other obligations of a public surface, so a small fix on `next` carried its handbook update, its decision record, and its marking with it.

That made the cost of landing a change on `next` several times the cost of the change. The only person who sees `next` is the maintainer who built it, so those obligations bought nothing at the point they were paid; they matter to whoever reads `main`.

## Decision

The obligations a public surface owes are discharged once per promotion, for the whole promoted range, and are not owed by a landing on `next`.

- **In-development marking moves to promotion.** A change may land on `next` with a half-built surface unmarked. The promotion marks every half-built surface in its range (the in-development register and its marker) or finishes it, on the promotion branch, before the merge. On `main` the rule is unchanged: nothing reads as finished or measured when it is not.
- **Documentation and decision records catch up at promotion**, as ADR-0029 already intended: the promotion updates the architecture chapters for every documented surface its range moved and records any significant decision it made, so `main` and the published handbook move together.
- **Preparation commits sit on the promotion branch, not on `next`.** A promotion can stop short of `next`'s tip; a commit on `next` would pull every later change into it. The preparation reaches `next` when `main` is next merged back into it.
- **What stays hard on `next`:** a build that passes, database migrations that are additive and forward-only, and the authoring lint on every push, because those protect the maintainer's data and a history that is permanent the moment it is pushed.

## Consequences

- Landing a change on `next` costs its own checks and a push; the continuous-integration run on `next` provides the full verification afterwards.
- A promotion is heavier than before: it surveys its range for unmarked surfaces and stale documentation and commits the corrections itself.
- `next` can show surfaces that are unmarked and documentation that is behind the code. Building from `main` gives the reviewed, documented state; building from `next` gives the latest work as it stands.
