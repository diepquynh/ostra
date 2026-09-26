#!/usr/bin/env bash
# Build the web UI (only when it changed) and the release binary at target/release/ostra.
set -euo pipefail
cd "$(dirname "$0")"

# The binary embeds web/dist at compile time, so the UI is built first.
# web/, design/, and site/ are one npm workspace, installed at the repository root.
if [[ ! -d node_modules ]]; then
  echo "==> Installing web dependencies"
  npm ci --no-audit --no-fund
fi
if [[ ! -f web/dist/index.html ]] || [[ -n "$(find web/src web/public web/index.html web/package.json design/src design/package.json -newer web/dist/index.html -print -quit 2>/dev/null)" ]]; then
  echo "==> Building the web UI"
  (cd web && npm run -s build)
else
  echo "==> Web UI is up to date"
fi
echo "==> Building ostra (release)"
cargo build --release -p ostra-server
