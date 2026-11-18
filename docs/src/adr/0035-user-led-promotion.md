# ADR-0035: Promotion is user-led, up to the point the maintainer has used

- Status: Accepted (supersedes the promotion cadence in [ADR-0029](0029-two-line-development.md))
- Context: ADR-0029 promoted `next` to `main` on a time-boxed cadence, every two weeks or before
  a release. Its purpose was that what reaches `main` has been lived with on `next` first. A
  calendar cannot tell whether that happened, and a cadence note prompting a promotion invites
  promoting work nobody has yet used.

## Context and problem statement

The maintainer's installed build tracks `next`, so every banked change is exercised in real
play there before anything else. That is the evidence a promotion is meant to carry. How much
of `next` has actually been used varies with how much the maintainer has played, not with
elapsed time: two weeks can pass with a feature untouched, and a feature can be settled within
days. A fixed interval both promotes too early and suggests an obligation where there is only
a judgement only the maintainer can make.

## Decision

- **No cadence.** Nothing is due. A promotion starts only when the maintainer says it is time.
- **The maintainer names the cut.** The promotion lists every change waiting on `next`, grouped
  as it landed, and the maintainer says how far they are confident, having used it: "up to
  here". The cut is the last change they name.
- **A promotion is a prefix of `next`.** It still merges by merge commit, so it carries every
  commit up to the cut and none after it. Changes are never picked individually from within
  `next`: a selection would give the two lines different commits for the same change and every
  later promotion would conflict. A single fix that cannot wait still reaches `main` as a
  squash hotfix pull request, as ADR-0029 describes.
- **Mechanics.** The promotion pull request's head is a short-lived branch at the cut, with
  `main` merged in so it is up to date; the branch is deleted once the promotion lands. Every
  other part of ADR-0029 stands: `CI gate` is required, auto-merge lands the pull request,
  review is asynchronous, and `next` is never rewritten.

## Consequences

`main` carries only what has been used in real play, by the maintainer's own account, and a
release carries only what has been promoted. `next` can run further ahead of `main` than
before; a larger promotion is reviewed as a larger batch, and an external review can skip a
batch past its file limit, which the promotion records when it happens. Work that must reach a
release before the maintainer has used everything ahead of it on `next` takes the hotfix path.
