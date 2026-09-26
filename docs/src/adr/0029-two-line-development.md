# ADR-0029: Two-line development with promotion after soak

- Status: Accepted (its promotion cadence superseded by [ADR-0035](0035-user-led-promotion.md))
- Context: a solo-maintained, versioned desktop application with explicit releases ([ADR-0014](0014-release-engineering.md): releases are cut from `main` by tag), where gating every change on the release line had become the bottleneck. This record leaves ADR-0014 in force and changes only how work reaches `main`.

## Context and problem statement

EntropiaOrme ships as explicit, versioned releases and is maintained by one person who also runs the application every day. Until this decision every change reached `main` through a pull request whose landing waited on the per-pull-request workflow, an external review pass, and a merge queue that re-ran the same workflow on the integrated commit before merging. The queue had been introduced when a heavy test tier ran only there, off the per-pull-request path; that tier was later folded into the ordinary workflow, after which the queue run duplicated the pull request's own run byte for byte.

The cost was paid per change, and small changes paid it in full: across the most recent pull requests, the fastest landing of a code change took over twenty minutes, nearly all of it waiting. The predictable effect of that activation energy is fewer, larger changes, each harder to review than the sum of the small ones it replaced.

The release model makes a second observation possible: `main` does not need to be where work first lands. A release is a tag on a vetted state, and what vetting means here is having run the change, installed, for long enough to trust it.

## Decision

Development moves to two lines with distinct roles.

**`next` is the integration line.** Work lands on it directly (a push, or a local branch merged locally and pushed) and is lived with there: the maintainer's installed build tracks `next`. The workflow runs on every push as information; a red run is fixed forward. Two of the trunk's rules stay hard on `next` because its history is as permanent as `main`'s: database migrations are additive and forward-only, and an unfinished surface is marked as such where it renders (the in-development register) rather than presented as complete.

**`main` is the vetted line.** It changes only through pull requests. A promotion pull request from `next` merges by merge commit on a time-boxed cadence: every two weeks, or before a release, whichever comes first. A hotfix that cannot wait for promotion lands as a squash pull request. The `CI gate` check is required at every commit, review happens on the promotion as a batch, and documentation and decision-record obligations are discharged there. The integration branch is never squashed into `main`: a squash would rewrite the commits `next` still carries and the two lines would diverge permanently.

**The merge queue is retired** and auto-merge takes its place. The queue's purpose was a pre-merge tier that no longer exists; the post-merge workflow run on `main` and the nightly campaign are the remaining net, and both already ran before this decision.

**Review is asynchronous and non-gating.** An external review is requested on promotion pull requests and wherever the author wants a second pair of eyes; its findings become follow-up commits on `next`. Required conversation resolution on `main` is dropped; the `CI gate` status check stays required at every commit.

**Deterministic lint replaces per-push human review of the repository's permanent record.** Because a push to `next` is as public as a merge to `main`, the authoring lint gains a dimension over commit messages and added prose: no references to files the repository does not have, no iteration tokens, and no tool-attribution lines. The rules are repository-relative hygiene, mechanically checked in the same job as the existing em-dash and spelling rules.

**Dependency updates target `next`**, so a bump soaks like every other change and reaches `main` by promotion.

## Consequences

- `main`'s first-parent history becomes a sequence of promotions, each a release-sized step; `git log --first-parent main` reads as the release narrative, while the merge preserves every underlying commit, so `git bisect` still works at commit granularity.
- Review at promotion is a second pass over work that has already been run, not the first look at it; the per-change review that gated the old flow becomes optional and asynchronous.
- The workflow runs twice for a landed change (on the push to `next`, then on the promotion) but never blocks the first landing; the retired queue run was a third, identical, run.
- A promotion pull request that falls behind `main` (a hotfix landed meanwhile) is refreshed by merging `main` into `next`, never by rebasing or squashing either line.
- Building from `main` gives the stable state; building from `next` gives the latest changes, at the cost of carrying whatever is in soak.
