#!/usr/bin/env bash
# Run the checked core replay through the repo-native Gradle real-client gate.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

export SOLARIS_REAL_CLIENT_MANIFEST="$REPO_ROOT/docs/real-client-regression/manifests/core-replay-seed-81.json"
export SOLARIS_REAL_CLIENT_AGENT_SCENARIO="core-actions-seed-81"
export SOLARIS_REAL_CLIENT_FRESH_WORLD="${SOLARIS_REAL_CLIENT_FRESH_WORLD:-1}"
export SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS="${SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS:-180}"

if [[ "$#" -eq 0 ]]; then
  set -- --run
fi

exec "$REPO_ROOT/tools/run-real-client-regression.sh" "$@"
