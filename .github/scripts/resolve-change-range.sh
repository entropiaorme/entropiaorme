#!/usr/bin/env bash
# Resolve the base..head range a diff-scoped guard inspects, from the workflow
# event, and say when there is nothing to inspect.
#
# A pull request supplies its base..head through PR_BASE_SHA / PR_HEAD_SHA. A
# push supplies the pushed commits' before..after through PUSH_BEFORE_SHA /
# PUSH_HEAD_SHA. A push that creates a branch has no before (git reports the
# all-zero SHA, and the event carries created=true), so the range is
# established from what the repository already holds instead: when the pushed
# head is already reachable from another remote branch, the push introduced no
# new commits and the guard skips; otherwise the range runs from the merge base
# with the default branch, so every commit the push introduced is inspected.
# Any other event, a handled event missing a SHA, and a created branch with no
# merge base are errors: the guards fail closed on a range they cannot resolve.
#
# Emits base, head, and skip (true/false) as step outputs. Run from a workflow
# step after a full-history checkout, from the repository root:
#   EVENT_NAME=push PUSH_BEFORE_SHA=<before> PUSH_HEAD_SHA=<after> \
#     PUSH_REF_NAME=<branch> bash .github/scripts/resolve-change-range.sh

set -euo pipefail

zero_sha="0000000000000000000000000000000000000000"
default_branch="${DEFAULT_BRANCH:-main}"

case "${EVENT_NAME:-}" in
  pull_request) base="${PR_BASE_SHA:-}"; head="${PR_HEAD_SHA:-}" ;;
  push) base="${PUSH_BEFORE_SHA:-}"; head="${PUSH_HEAD_SHA:-}" ;;
  *) base=""; head="" ;;
esac

emit() {
  # base head skip
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    {
      echo "base=$1"
      echo "head=$2"
      echo "skip=$3"
    } >> "$GITHUB_OUTPUT"
  fi
}

if [ -z "$head" ]; then
  echo "resolve-change-range: no head commit for event '${EVENT_NAME:-}'; failing closed." >&2
  exit 1
fi

if [ "${PUSH_CREATED:-false}" = "true" ] || [ "$base" = "$zero_sha" ]; then
  # Reachable from any other remote branch: nothing new was introduced.
  if git branch -r --contains "$head" 2>/dev/null \
      | sed 's/^[* ]*//' \
      | grep -v -x "origin/${PUSH_REF_NAME:-}" \
      | grep -q .; then
    echo "resolve-change-range: the push created its branch from commits already on another branch; nothing new to inspect, the diff-scoped guard is skipped."
    emit "" "$head" true
    exit 0
  fi
  base="$(git merge-base "$head" "origin/$default_branch" 2>/dev/null || true)"
  if [ -z "$base" ]; then
    echo "resolve-change-range: the push created its branch with commits that share no history with $default_branch; failing closed." >&2
    exit 1
  fi
  echo "resolve-change-range: the push created its branch; inspecting from its merge base with $default_branch."
fi

if [ -z "$base" ]; then
  echo "resolve-change-range: no base commit for event '${EVENT_NAME:-}'; failing closed." >&2
  exit 1
fi

if ! git cat-file -e "$base^{commit}" 2>/dev/null; then
  echo "resolve-change-range: base commit $base is not in the checkout (was the ref rewritten?); failing closed." >&2
  exit 1
fi

echo "resolve-change-range: $base..$head"
emit "$base" "$head" false
