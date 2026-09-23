#!/usr/bin/env bash
# Build the web UI and the release binary, load credentials, and start Ostra.
# Usage: ./run.sh [ostra flags...]     for example: ./run.sh --bind 0.0.0.0
# Environment:
#   OSTRA_ENV_FILE   credentials file whose `export NAME=value` lines are applied, when set
#   SKIP_BUILD=1     start the existing binary without building
set -euo pipefail
cd "$(dirname "$0")"

if [[ "${SKIP_BUILD:-}" != "1" ]]; then
  # The binary embeds web/dist at compile time, so the UI is built first, and only when it changed.
  if [[ ! -d web/node_modules ]]; then
    echo "==> Installing web dependencies"
    (cd web && npm ci --no-audit --no-fund)
  fi
  if [[ ! -f web/dist/index.html ]] || [[ -n "$(find web/src web/public web/index.html web/package.json -newer web/dist/index.html -print -quit 2>/dev/null)" ]]; then
    echo "==> Building the web UI"
    (cd web && npm run -s build)
  else
    echo "==> Web UI is up to date"
  fi
  echo "==> Building ostra (release)"
  cargo build --release -p ostra-server
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
