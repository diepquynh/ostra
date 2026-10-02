#!/usr/bin/env bash
# Cut a release: compute the next version from the commit titles since the last tag, set it in every
# manifest, write CHANGELOG.md, commit, and tag. Pushing is left to you.
# Usage: ./release.sh [X.Y.Z]     the argument overrides the computed version
# Needs git-cliff (cargo install git-cliff --locked) and npm.
set -euo pipefail
cd "$(dirname "$0")"

command -v git-cliff >/dev/null || { echo "Install git-cliff first: cargo install git-cliff --locked" >&2; exit 1; }
if [[ -n "$(git status --porcelain)" ]]; then
  echo "Commit or stash your changes first, because the release commit must hold only the version bump." >&2
  exit 1
fi

next="${1:-$(git cliff --bumped-version)}"
next="${next#v}"
if ! [[ "$next" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Give a version as X.Y.Z, not '$next'." >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/v$next" >/dev/null; then
  echo "Tag v$next already exists, so there is nothing new to release." >&2
  exit 1
fi

echo "==> Releasing $next"
sed -i.bak -E "/^\[workspace\.package\]/,/^\[/ s/^version = \".*\"/version = \"$next\"/" Cargo.toml
rm Cargo.toml.bak
cargo update --workspace --quiet
npm version "$next" --workspaces --include-workspace-root=false --no-git-tag-version --allow-same-version >/dev/null
git cliff --tag "v$next" --output CHANGELOG.md

git add Cargo.toml Cargo.lock package-lock.json design/package.json web/package.json site/package.json CHANGELOG.md
git commit --quiet -m "chore: Release v$next"
git tag -a "v$next" -m "Ostra $next"
echo "==> Tagged v$next. Publish it with: git push origin HEAD v$next"
