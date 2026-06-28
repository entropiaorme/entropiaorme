# Ratification artefacts

This directory holds the recorded adversarial-review verdicts that accompany
changes to the testing oracle's expected output (a regenerated golden, or the
first pin of a newly emitted one).

When a change moves a golden file, the `golden-ratification` pull-request job
(`backend/scripts/check_golden_ratification.py`) requires, alongside the
`test: regenerate goldens` commit-message marker, a report committed here as
`<slug>.md` and committed no earlier than the last golden change in that range.
The report ends in a fenced verdict block the guard parses (the block must be
fenced; the parser reads only the fenced block, never the surrounding prose):

```text
ORACLE-RATIFICATION
range: <commit-range>
goldens: <comma-separated sets reviewed>
VERDICT: ratification-sound | regression-suspected | needs-user-judgement
```

The gate passes only on `VERDICT: ratification-sound`, only when the `goldens:`
field names every changed set, and only when the report was not left stale by a
later same-range golden edit. See the "Adversarial ratification"
and "Ratification guard" sections of `TESTING.md` for the full flow and the
rationale (the structural conflict of interest a self-approved golden move
carries, and why a committed report is required over a bare commit trailer).

These reports are deliberately kept **outside** any `expected/` directory so the
guard never classifies a ratification artefact as a golden and demands a
ratification of the ratification.
