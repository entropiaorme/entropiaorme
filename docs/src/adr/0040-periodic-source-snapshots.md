# ADR-0040: Periodic source snapshots replace two-line development

- Status: Accepted
- Context: supersedes [ADR-0029](0029-two-line-development.md) and [ADR-0035](0035-user-led-promotion.md); amends [ADR-0038](0038-promotion-carries-public-polish.md) and [ADR-0014](0014-release-engineering.md).

## Context and problem statement

EntropiaOrme is a personal application: its maintainer builds it for their own play, and the public repository is read by developers and players for ideas, not installed by users. ADR-0029 nonetheless ran development in public on two lines, with every change pushed to `next` and periodically promoted to `main`, and ADR-0014 built a release pipeline of installers, a signed self-update channel, and provenance attestations around it.

That arrangement asked a one-person project to perform a public development process continuously: every commit message, comment, and half-finished surface was written for an audience at the moment it was made. The installers and the update channel served no measurable user base while requiring a signing story, a distribution site, and a pipeline to maintain.

## Decision

Development moves to a private working tree, and this repository receives an occasional source snapshot.

- **`main` is the only line.** `next` is retired after a final promotion. Each snapshot arrives as one commit on `main` by pull request, under the same `CI gate` check as before.
- **A snapshot is cut when the maintainer has lived with the changes**, as promotions were under ADR-0035. There is no cadence.
- **The obligations of a public surface are discharged once per snapshot**, as ADR-0038 discharged them once per promotion: unfinished surfaces are marked or finished, and the handbook and decision records are brought up to date, in the snapshot that carries the change.
- **The repository is source-only.** No installers, releases, or update manifests are published. The tag-driven release workflow, the documentation-site and distribution-site workflows, the scheduled mutation campaign, and automated dependency updates are removed. Existing releases remain as historical artefacts and are not maintained.

## Consequences

- The history before this decision is the complete development history. After it, the public history is one commit per snapshot, and per-change history is not published.
- A snapshot's pull request is the only run of the full workflow, including the Windows jobs and the coverage measurement, that a change receives in this repository.
- The in-application updater and the release machinery described in ADR-0014 no longer have anything to serve; their removal from the code follows in a later snapshot.
- Building from source is the only way to run the application.
