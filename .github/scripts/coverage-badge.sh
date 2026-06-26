#!/usr/bin/env bash
# Turn a cargo-llvm-cov JSON export into a shields.io endpoint badge JSON.
#
# The Rust backend members' branch coverage is already measured in CI by the
# `rust-coverage` job (`cargo llvm-cov ... --branch`); this script reduces that
# job's JSON export to the small endpoint document the README's Coverage badge
# points shields.io at, published to the `badges` branch by publish-badge.sh.
# It badges BRANCH coverage (the figure the job exists to produce), so the label
# says so and a reader cannot mistake it for the usually-higher line coverage.
#
# The shape and colour bands mirror backend/scripts/mutation_score.py so the two
# product badges read consistently.
#
# Usage: coverage-badge.sh <llvm-cov-json-export> <out-json>
#   <llvm-cov-json-export>  output of `cargo llvm-cov report --json --branch`
#   <out-json>              shields.io endpoint badge JSON to write
set -euo pipefail

if [[ $# -ne 2 ]]; then
	echo "Usage: $0 <llvm-cov-json-export> <out-json>" >&2
	exit 2
fi

src="$1"
out="$2"

# Branch coverage percentage from the llvm-cov export totals. `-e` makes jq exit
# non-zero (failing the step) if the field is absent rather than emitting null,
# so a changed export shape is caught loudly instead of badging "null%".
pct="$(jq -er '.data[0].totals.branches.percent' "$src")"

# One-decimal message, matching the mutation badge's `f"{score:.1f}%"`.
message="$(awk -v p="$pct" 'BEGIN { printf "%.1f%%", p }')"

# Shields colour band, identical floors to mutation_score._colour.
color="$(awk -v p="$pct" 'BEGIN {
	if (p >= 90) print "brightgreen"
	else if (p >= 80) print "green"
	else if (p >= 70) print "yellowgreen"
	else if (p >= 60) print "yellow"
	else if (p >= 50) print "orange"
	else print "red"
}')"

jq -n \
	--arg label "branch coverage" \
	--arg message "$message" \
	--arg color "$color" \
	'{schemaVersion: 1, label: $label, message: $message, color: $color}' \
	>"$out"

echo "Wrote coverage badge ($message, $color) to $out"
