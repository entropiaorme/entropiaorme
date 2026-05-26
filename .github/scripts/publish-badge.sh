#!/usr/bin/env bash
# Publish a shields.io endpoint badge JSON to the repository's `badges` branch.
#
# The badges branch is an orphan data branch that holds only these small JSON
# files; the README points shields.io at their raw URLs. Keeping the data in a
# branch of this repository (rather than a third-party badge service) keeps the
# metric in-repo and needs nothing beyond the workflow's own token.
#
# Usage: publish-badge.sh <json-file>
# Requires GITHUB_TOKEN (contents: write) and GITHUB_REPOSITORY in the
# environment, both provided to GitHub Actions jobs by default.
set -euo pipefail

src="$1"
name="$(basename "$src")"
# Default to the token-authenticated GitHub remote; BADGE_REMOTE_URL overrides it
# (used by the script's own test against a local repository).
remote="${BADGE_REMOTE_URL:-https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPOSITORY}.git}"

git config --global user.name "github-actions[bot]"
git config --global user.email "41898282+github-actions[bot]@users.noreply.github.com"

work="$(mktemp -d)"
if git ls-remote --exit-code "$remote" badges >/dev/null 2>&1; then
	# Existing branch: take only its tip so a fresh badge layers onto it without
	# disturbing the other JSON files already published there.
	git clone --depth 1 --branch badges --single-branch "$remote" "$work"
else
	git clone --depth 1 "$remote" "$work"
	git -C "$work" checkout --orphan badges
	git -C "$work" rm -rf . >/dev/null 2>&1 || true
fi

cp "$src" "$work/$name"
git -C "$work" add "$name"
if git -C "$work" diff --cached --quiet; then
	echo "Badge $name unchanged; nothing to publish."
	exit 0
fi
git -C "$work" commit -m "chore: update $name"
git -C "$work" push "$remote" HEAD:badges
echo "Published $name to the badges branch."
