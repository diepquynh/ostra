#!/usr/bin/env bash
# Build the web UI and the release binary, load credentials, and start Ostra.
# Usage: ./run.sh [ostra flags...]     for example: ./run.sh --bind 0.0.0.0
# Environment:
#   OSTRA_ENV_FILE   credentials file whose `export NAME=value` lines are applied, when set
#   SKIP_BUILD=1     start the existing binary without building
set -euo pipefail
cd "$(dirname "$0")"

if [[ "${SKIP_BUILD:-}" != "1" ]]; then
  ./build.sh
fi

env_file="${OSTRA_ENV_FILE:-}"
if [[ -f "$env_file" ]]; then
  # Only the `export NAME=value` lines are applied, so nothing else in the file runs.
  echo "==> Loading exported variables from $env_file"
  while IFS= read -r line; do
    eval "$line"
  done < <(grep -E '^[[:space:]]*export[[:space:]]+[A-Za-z_][A-Za-z0-9_]*=' "$env_file")
fi

echo "==> Starting ostra $*"
exec ./target/release/ostra "$@"
