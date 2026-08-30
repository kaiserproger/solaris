#!/usr/bin/env bash
set -euo pipefail

EXPECTED_VERSION="0.30.0"
PRISTINE_SERVER="d289c1661455b6ea4df7a5b1c148dc750c819e3ed7ea228ca70db5597e80c877"
PRISTINE_SMOKE="80645141978e9c6a51adf65676b1b3100625b979565bb6a2ba8806b70e4027c4"
BASE_SERVER="8b6088ac9110257fe0bed2e5859f652e1defa54ba3bcd2a4ffd2a123ab7dc387"
BASE_PI="983f7ec10fad5f264a0631880c4978af4184594b3a13d491e583140485f894d4"
BASE_SMOKE="cfaa886989e5c667810becb443552aa16bb5d43c330cccfdb40bafa3bd026146"
FINAL_SERVER="006e700f5ab78496b3bfae5462df81945dcc2ee29881d5cd96fd33002346090b"
FINAL_PI="50af2f100bdb45020d778032d241f6141b1e9faf7dfa9576b4f6b62a4f09898c"
FINAL_SMOKE="$BASE_SMOKE"

ROOT="$(git rev-parse --show-toplevel)"
BASE_PATCH="$ROOT/tools/codexpro-pi-overlay/codexpro-0.30.0.patch"
HOTFIX_PATCH="$ROOT/tools/codexpro-pi-overlay/codexpro-0.30.0-workspace-routing.patch"
ROUTING_SMOKE="$ROOT/tools/codexpro-pi-overlay/workspace-routing-smoke.mjs"
CODEXPRO_BIN="$(command -v codexpro)"
CODEXPRO_SCRIPT="$(readlink -f "$CODEXPRO_BIN")"
CODEXPRO_ROOT="$(cd "$(dirname "$CODEXPRO_SCRIPT")/.." && pwd)"

VERSION="$(node -e 'const p=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); process.stdout.write(String(p.version))' "$CODEXPRO_ROOT/package.json")"
if [[ "$VERSION" != "$EXPECTED_VERSION" ]]; then
  echo "refusing CodexPro overlay: expected $EXPECTED_VERSION, found $VERSION at $CODEXPRO_ROOT" >&2
  exit 2
fi

sha() {
  sha256sum "$1" | awk '{print $1}'
}

runtime_hashes() {
  server_sha="$(sha "$CODEXPRO_ROOT/dist/server.js")"
  smoke_sha="$(sha "$CODEXPRO_ROOT/scripts/smoke.mjs")"
  pi_sha="MISSING"
  if [[ -f "$CODEXPRO_ROOT/dist/piAgentOps.js" ]]; then
    pi_sha="$(sha "$CODEXPRO_ROOT/dist/piAgentOps.js")"
  fi
}

runtime_hashes
if [[ "$server_sha" == "$FINAL_SERVER" && "$smoke_sha" == "$FINAL_SMOKE" && "$pi_sha" == "$FINAL_PI" ]]; then
  echo "CodexPro Pi overlay + workspace-routing hotfix already applied to $CODEXPRO_ROOT"
else
  if [[ "$server_sha" == "$PRISTINE_SERVER" && "$smoke_sha" == "$PRISTINE_SMOKE" && "$pi_sha" == "MISSING" ]]; then
    patch --batch --forward -p1 -d "$CODEXPRO_ROOT" < "$BASE_PATCH"
    runtime_hashes
  fi

  if [[ "$server_sha" != "$BASE_SERVER" || "$smoke_sha" != "$BASE_SMOKE" || "$pi_sha" != "$BASE_PI" ]]; then
    cat >&2 <<EOF
refusing CodexPro overlay: installation is neither pristine, the expected base Pi overlay, nor the final workspace-routing build
  server=$server_sha
  pi=$pi_sha
  smoke=$smoke_sha
Rebase/regenerate the overlay for this CodexPro build instead of patching blindly.
EOF
    exit 3
  fi

  patch --batch --forward -p1 -d "$CODEXPRO_ROOT" < "$HOTFIX_PATCH"
fi

[[ "$(sha "$CODEXPRO_ROOT/dist/server.js")" == "$FINAL_SERVER" ]]
[[ "$(sha "$CODEXPRO_ROOT/dist/piAgentOps.js")" == "$FINAL_PI" ]]
[[ "$(sha "$CODEXPRO_ROOT/scripts/smoke.mjs")" == "$FINAL_SMOKE" ]]
node --check "$CODEXPRO_ROOT/dist/server.js"
node --check "$CODEXPRO_ROOT/dist/piAgentOps.js"
(
  cd "$CODEXPRO_ROOT"
  node scripts/smoke.mjs
)
node "$ROUTING_SMOKE" "$CODEXPRO_ROOT"

echo "CodexPro Pi overlay validated. Restart the running CodexPro process before expecting clients to discover repo_root/repo_subdir on Pi tools."
