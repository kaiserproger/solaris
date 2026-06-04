#!/usr/bin/env bash
# Prepare a local-only evidence directory for an M78 real-client run.
# This wrapper does not count protocol bots or mocks as client evidence.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIO="${M78_SCENARIO:-$REPO_ROOT/tools/client-automation/scenarios/m78_smoke.json}"
RUN_ROOT="${M78_RUN_ROOT:-$REPO_ROOT/.analysis/client-automation/runs}"
MODE="prepare"

usage() {
  cat <<'EOF'
Usage: tools/prepare-real-client-scenario.sh [--check|--prepare]

Environment:
  M78_SCENARIO        Scenario manifest path. Defaults to M78 smoke scenario.
  M78_RUN_ROOT        Local-only run root. Defaults to .analysis/client-automation/runs.
  M78_CLIENT_COMMAND  Real vanilla/PrismLauncher client launch command.

Evidence status:
  --check exits non-zero when a real-client command is absent. That is a
  prepared-owner-run or blocked gate, not green client evidence.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      MODE="check"
      ;;
    --prepare)
      MODE="prepare"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

require_file() {
  if [[ ! -f "$1" ]]; then
    printf 'error: missing required file: %s\n' "$1" >&2
    exit 1
  fi
}

require_file "$SCENARIO"
require_file "$REPO_ROOT/example.toml"

client_command_lc="${M78_CLIENT_COMMAND:-}"
client_command_lc="${client_command_lc,,}"

if [[ "$client_command_lc" == *wire-probe* || "$client_command_lc" == *mc-test-harness* ]]; then
  printf 'error: M78_CLIENT_COMMAND must launch a real client, not a protocol bot\n' >&2
  exit 1
fi

client_gate_status='prepared: scenario and plausible real-client command are configured; no client evidence has been run'
client_gate_exit=0

if [[ -z "${M78_CLIENT_COMMAND:-}" ]]; then
  client_gate_status='degraded: M78_CLIENT_COMMAND is unset; prepared owner-run only'
  client_gate_exit=1
elif [[ "$client_command_lc" != *prism* && "$client_command_lc" != *minecraft* && "$client_command_lc" != *java* ]]; then
  client_gate_status='degraded: M78_CLIENT_COMMAND is set but does not look like a vanilla/PrismLauncher client command'
  client_gate_exit=1
fi

if [[ "$client_gate_exit" -ne 0 && "$MODE" != "check" ]]; then
  printf '%s\n' "$client_gate_status" >&2
fi

if [[ "$MODE" == "check" ]]; then
  printf '%s\n' "$client_gate_status"
  exit "$client_gate_exit"
fi

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
scenario_id="$(basename "$SCENARIO" .json)"
run_dir="$RUN_ROOT/$timestamp-$scenario_id"
mkdir -p "$run_dir/screenshots"

git_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)"
git_branch="$(git -C "$REPO_ROOT" branch --show-current)"
scenario_sha256="$(sha256sum "$SCENARIO" | cut -d ' ' -f 1)"

{
  printf 'scenario=%s\n' "$SCENARIO"
  printf 'scenario_sha256=%s\n' "$scenario_sha256"
  printf 'git_commit=%s\n' "$git_commit"
  printf 'git_branch=%s\n' "$git_branch"
  printf 'config=%s\n' "$REPO_ROOT/example.toml"
  printf 'server_command=%s\n' 'cargo run --bin mc-server -- --config example.toml'
  printf 'client_command=%s\n' "${M78_CLIENT_COMMAND:-UNSET_PREPARED_OWNER_RUN}"
  printf 'rustc=%s\n' "$(rustc --version)"
  printf 'cargo=%s\n' "$(cargo --version)"
  printf 'java=%s\n' "$(java --version 2>&1 | tr '\n' ' ' | sed 's/[[:space:]]*$//')"
  printf 'client_gate=%s\n' "$client_gate_status"
} > "$run_dir/run-manifest.txt"

cat > "$run_dir/observations.md" <<EOF
# M78 Real-Client Smoke Observations

Quality label: draft.
Client gate: prepared owner-run until a real vanilla 26.1.2 client fills this file.

## Commands

- Server: \`cargo run --bin mc-server -- --config example.toml\`
- Client: \`${M78_CLIENT_COMMAND:-UNSET_PREPARED_OWNER_RUN}\`

## Steps

Record each scenario step from \`$SCENARIO\` here with pass/fail notes,
screenshots copied into \`screenshots/\`, and client/server log paths.

## Result

- join:
- wait_for_chunks:
- move:
- break_place_block:
- open_close_container:
- disconnect:
EOF

cat > "$run_dir/OWNER_STEPS.md" <<EOF
# Owner-Run M78 Real Client Path

1. Start Solaris from the repo root and tee server output to this run dir:
   \`cargo run --bin mc-server -- --config example.toml 2>&1 | tee '$run_dir/server.log'\`
2. Launch a real vanilla 26.1.2 client or PrismLauncher instance. If using this wrapper, set:
   \`M78_CLIENT_COMMAND='<real client command>' bash tools/prepare-real-client-scenario.sh --check\`
3. Connect to \`127.0.0.1:25565\` and execute \`$SCENARIO\`.
4. Copy the client log to \`$run_dir/client.log\` and screenshots to \`$run_dir/screenshots/\`.
5. Fill \`$run_dir/observations.md\` with exact pass/fail notes.

Headless mocks, protocol-only bots, and \`wire-probe\` output are not real-client evidence for this gate.
EOF

printf 'prepared %s\n' "$run_dir"
