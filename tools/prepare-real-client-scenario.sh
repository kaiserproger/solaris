#!/usr/bin/env bash
# Prepare a local-only evidence directory for an M78 real-client run.
# This wrapper does not count protocol bots or mocks as client evidence.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIO="${M78_SCENARIO:-$REPO_ROOT/tools/client-automation/scenarios/m78_smoke.json}"
RUN_ROOT="${M78_RUN_ROOT:-$REPO_ROOT/.analysis/client-automation/runs}"
AGENT_ROOT="$REPO_ROOT/client-mod/solaris-client-agent"
GRADLE_RUNCLIENT_TASK=":fabric-agent:runClientAgent"
CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
MODE="prepare"

usage() {
  cat <<'EOF'
Usage: tools/prepare-real-client-scenario.sh [--check|--prepare]

Environment:
  M78_SCENARIO        Scenario manifest path. Defaults to M78 smoke scenario.
  M78_RUN_ROOT        Local-only run root. Defaults to .analysis/client-automation/runs.

Evidence status:
  --check exits non-zero when the repo-native Gradle runClient adapter is
  absent. Prepared directories are not green client evidence.
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

gradle_runclient_adapter_available() {
  [[ -x "$AGENT_ROOT/gradlew" && -f "$AGENT_ROOT/fabric-agent/build.gradle.kts" ]] \
    && grep -q 'create("clientAgent")' "$AGENT_ROOT/fabric-agent/build.gradle.kts"
}

client_gate_status='prepared: repo-native Gradle runClient adapter is configured; no client evidence has been run'
client_gate_exit=0

if ! gradle_runclient_adapter_available; then
  client_gate_status='degraded: repo-native Gradle runClient adapter is missing; prepared owner-run only'
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
  printf 'client_kind=%s\n' 'gradle-runclient'
  printf 'client_adapter_source=%s\n' "$CLIENT_ADAPTER_SOURCE"
  printf 'client_adapter_root=%s\n' "$AGENT_ROOT"
  printf 'client_adapter_gradlew=%s\n' "$AGENT_ROOT/gradlew"
  printf 'client_adapter_task=%s\n' "$GRADLE_RUNCLIENT_TASK"
  printf 'client_adapter_args=%s\n' "--no-configuration-cache $GRADLE_RUNCLIENT_TASK"
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
- Client adapter: \`client-mod/solaris-client-agent/gradlew --no-configuration-cache :fabric-agent:runClientAgent\`

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
2. Check that the repo-native Gradle adapter is available:
   \`bash tools/prepare-real-client-scenario.sh --check\`
3. Launch the real client through the Gradle adapter:
   \`client-mod/solaris-client-agent/gradlew --no-configuration-cache :fabric-agent:runClientAgent\`
4. Connect to \`127.0.0.1:25565\` and execute \`$SCENARIO\`.
5. Copy the client log to \`$run_dir/client.log\` and screenshots to \`$run_dir/screenshots/\`.
6. Fill \`$run_dir/observations.md\` with exact pass/fail notes.

Headless mocks, protocol-only bots, and \`wire-probe\` output are not real-client evidence for this gate.
EOF

printf 'prepared %s\n' "$run_dir"
