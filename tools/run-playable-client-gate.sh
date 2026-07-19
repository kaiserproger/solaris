#!/usr/bin/env bash
# Run the playable real-client gate with fail-closed defaults.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

export SOLARIS_REAL_CLIENT_MANIFEST="${SOLARIS_REAL_CLIENT_MANIFEST:-$REPO_ROOT/docs/playable/real-client-playable-loop.json}"
export SOLARIS_REAL_CLIENT_SERVER_CONFIG="${SOLARIS_REAL_CLIENT_SERVER_CONFIG:-playable.toml}"
export SOLARIS_REAL_CLIENT_FRESH_WORLD="${SOLARIS_REAL_CLIENT_FRESH_WORLD:-1}"
export SOLARIS_REAL_CLIENT_AGENT_SCENARIO="${SOLARIS_REAL_CLIENT_AGENT_SCENARIO:-playable-04-twenty-minute-survival-loop}"
export SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS="${SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS:-1500}"

if [[ "$#" -eq 0 ]]; then
  set -- --run
fi

exec "$REPO_ROOT/tools/run-real-client-regression.sh" "$@"
