#!/usr/bin/env bash
# Classify a change as documentation-only or code, to gate the expensive CI jobs.
#
# The per-pull-request CI gate runs a Windows backend test matrix and a frontend
# build, and the full tier runs on the merge queue's integrated commit before a
# change lands. A change that touches only documentation (Markdown) needs none of
# that. This guard inspects the set of files a change touches and emits a single
# `code` flag the workflows read to decide whether to run those jobs.
#
# The flag is deliberately conservative. `code=false` (documentation-only: skip
# the expensive jobs) is emitted only when EVERY changed path is a Markdown file.
# Any other path, an empty change set, an event that supplies no comparable range
# (such as a push to main), and any classification doubt all resolve to
# `code=true`, so the safe failure direction is to run the suite.
#
# The flag gates required checks, so the workflows pair it with a fail-closed
# aggregator: a documentation-only skip passes the gate, but a detection that did
# not run cleanly fails it, so a misfire can never let an untested code change
# through.
#
# Range resolution mirrors the workflow event: a pull request supplies its
# base..head through PR_BASE_SHA / PR_HEAD_SHA; a merge_group event supplies the
# integrated commit's base..head through MERGE_GROUP_BASE_SHA /
# MERGE_GROUP_HEAD_SHA. Any other event, or a handled event missing either SHA,
# yields no range and therefore code=true. An explicit --range overrides the env.
#
# Pure git + POSIX shell, so CI needs no language runtime. Run from a workflow:
#   EVENT_NAME=pull_request PR_BASE_SHA=<base> PR_HEAD_SHA=<head> \
#     bash .github/scripts/classify-change-scope.sh --repo-root "$GITHUB_WORKSPACE"
# or locally against an explicit range:
#   bash .github/scripts/classify-change-scope.sh --range origin/main..HEAD

set -euo pipefail

repo_root="."
commit_range=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo-root) repo_root="$2"; shift 2 ;;
    --range) commit_range="$2"; shift 2 ;;
    *) echo "classify-change-scope: unknown argument '$1'" >&2; exit 2 ;;
  esac
done

# Derive the range from the event env when not given explicitly.
if [ -z "$commit_range" ]; then
  case "${EVENT_NAME:-}" in
    pull_request) base="${PR_BASE_SHA:-}"; head="${PR_HEAD_SHA:-}" ;;
    merge_group) base="${MERGE_GROUP_BASE_SHA:-}"; head="${MERGE_GROUP_HEAD_SHA:-}" ;;
    *) base=""; head="" ;;
  esac
  if [ -n "$base" ] && [ -n "$head" ]; then
    commit_range="$base..$head"
  fi
fi

emit() {
  # value is the first argument: true or false.
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    echo "code=$1" >> "$GITHUB_OUTPUT"
  fi
}

# No comparable range: run the jobs unconditionally.
if [ -z "$commit_range" ]; then
  echo "classify-change-scope: no pull-request or merge-queue range to inspect; code=true (run the jobs)."
  emit true
  exit 0
fi

# The repo-relative paths the range touches (added / modified / deleted).
mapfile -t paths < <(git -C "$repo_root" diff --name-only "$commit_range" | sed '/^$/d')

# Documentation-only iff the change touches at least one path and every path is
# a Markdown file (case-insensitive). An empty set is treated as code.
code="true"
if [ "${#paths[@]}" -gt 0 ]; then
  docs_only="true"
  for p in "${paths[@]}"; do
    case "$(printf '%s' "$p" | tr '[:upper:]' '[:lower:]')" in
      *.md) ;;
      *) docs_only="false"; break ;;
    esac
  done
  if [ "$docs_only" = "true" ]; then
    code="false"
  fi
fi

echo "classify-change-scope: range $commit_range; code=$code."
emit "$code"
exit 0
