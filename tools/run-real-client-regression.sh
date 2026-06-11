#!/usr/bin/env bash
# Prepare or run the M94+ real-client regression pack.
# This path is intentionally fail-closed: protocol bots and mocks are rejected,
# and prepared artifacts do not count as passed client evidence.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="${SOLARIS_REAL_CLIENT_MANIFEST:-$REPO_ROOT/docs/real-client-regression/manifests/m94-regression-pack.json}"
RUN_ROOT="${SOLARIS_REAL_CLIENT_RUN_ROOT:-$REPO_ROOT/.analysis/real-client-runs}"
CLIENT_COMMAND="${SOLARIS_REAL_CLIENT_COMMAND:-${M94_CLIENT_COMMAND:-}}"
CLIENT_KIND="${SOLARIS_REAL_CLIENT_KIND:-}"
CLIENT_TIMEOUT_SECONDS="${SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS:-180}"
SERVER_START_SECONDS="${SOLARIS_REAL_CLIENT_SERVER_START_SECONDS:-8}"
SERVER_CONFIG="${SOLARIS_REAL_CLIENT_SERVER_CONFIG:-example.toml}"
MODE="prepare"
VALIDATE_RUN_DIR=""

usage() {
  cat <<'EOF'
Usage: tools/run-real-client-regression.sh [--check|--prepare|--run|--validate-run DIR]

Environment:
  SOLARIS_REAL_CLIENT_COMMAND   Real vanilla/PrismLauncher client command.
  M94_CLIENT_COMMAND            Back-compatible alias for SOLARIS_REAL_CLIENT_COMMAND.
  SOLARIS_REAL_CLIENT_KIND      One of: prism-launcher, vanilla-launcher, vanilla-client.
  SOLARIS_REAL_CLIENT_MANIFEST  Regression pack manifest path.
  SOLARIS_REAL_CLIENT_RUN_ROOT  Local-only artifact root. Defaults to .analysis/real-client-runs.
  SOLARIS_REAL_CLIENT_SERVER_CONFIG
                                Server config path. Defaults to example.toml.
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS
                                Timeout for --run client command. Defaults to 180.

Modes:
  --check         Validate that the configured command is allowed for real-client evidence.
  --prepare       Create a local run directory and observation templates.
  --run           Start Solaris, execute the configured real-client command, and record logs.
  --validate-run  Check that a run directory has the required artifact shape.

Protocol bots, wire-probe, mc-test-harness clients, and mocks are rejected.
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
    --run)
      MODE="run"
      ;;
    --validate-run)
      MODE="validate-run"
      shift
      if [[ $# -eq 0 ]]; then
        printf 'error: --validate-run requires a run directory\n' >&2
        exit 2
      fi
      VALIDATE_RUN_DIR="$1"
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

require_dir() {
  if [[ ! -d "$1" ]]; then
    printf 'error: missing required directory: %s\n' "$1" >&2
    exit 1
  fi
}

server_config_path() {
  if [[ "$SERVER_CONFIG" = /* ]]; then
    printf '%s\n' "$SERVER_CONFIG"
  else
    printf '%s/%s\n' "$REPO_ROOT" "$SERVER_CONFIG"
  fi
}

client_command_status() {
  local command_lc kind_lc
  command_lc="${CLIENT_COMMAND,,}"
  kind_lc="${CLIENT_KIND,,}"

  if [[ -z "$CLIENT_COMMAND" ]]; then
    printf 'degraded: SOLARIS_REAL_CLIENT_COMMAND is unset; prepared owner-run only\n'
    return 1
  fi
  if [[ "$command_lc" == *wire-probe* || "$command_lc" == *mc-test-harness* || "$command_lc" == *mc_test_harness* || "$command_lc" == *protocol-only* || "$command_lc" == *mock* ]]; then
    printf 'blocked: real-client command must not be a protocol bot or mock\n'
    return 1
  fi
  if [[ "$kind_lc" != "prism-launcher" && "$kind_lc" != "vanilla-launcher" && "$kind_lc" != "vanilla-client" ]]; then
    printf 'degraded: SOLARIS_REAL_CLIENT_KIND must be prism-launcher, vanilla-launcher, or vanilla-client\n'
    return 1
  fi
  if [[ "$command_lc" != *prism* && "$command_lc" != *minecraft* && "$command_lc" != *launcher* && "$command_lc" != *java* ]]; then
    printf 'degraded: command does not look like a vanilla/PrismLauncher client command\n'
    return 1
  fi

  printf 'agent-run real-client configured: %s\n' "$kind_lc"
  return 0
}

write_run_templates() {
  local run_dir status manifest_sha
  run_dir="$1"
  status="$2"
  manifest_sha="$(sha256sum "$MANIFEST" | cut -d ' ' -f 1)"

  mkdir -p "$run_dir/screenshots"
  cp "$MANIFEST" "$run_dir/manifest.json"

  {
    printf 'branch=%s\n' "$(git -C "$REPO_ROOT" branch --show-current)"
    printf 'commit=%s\n' "$(git -C "$REPO_ROOT" rev-parse HEAD)"
    printf 'status_short_begin\n'
    git -C "$REPO_ROOT" status --short --branch
    printf 'status_short_end\n'
    printf 'manifest=%s\n' "$MANIFEST"
    printf 'manifest_sha256=%s\n' "$manifest_sha"
  } > "$run_dir/git.txt"

  {
    printf 'rustc=%s\n' "$(rustc --version)"
    printf 'cargo=%s\n' "$(cargo --version)"
    printf 'java=%s\n' "$(java --version 2>&1 | tr '\n' ' ')"
  } > "$run_dir/toolchain.txt"

  {
    printf 'client_gate=%s\n' "$status"
    printf 'client_kind=%s\n' "${CLIENT_KIND:-UNSET}"
    if [[ -n "$CLIENT_COMMAND" ]]; then
      printf 'client_command=redacted\n'
      printf 'client_command_sha256=%s\n' "$(printf '%s' "$CLIENT_COMMAND" | sha256sum | cut -d ' ' -f 1)"
    else
      printf 'client_command=UNSET_PREPARED_OWNER_RUN\n'
    fi
    printf 'server_command=%s\n' "cargo run --bin mc-server -- --config $(server_config_path)"
    printf 'forbidden_clients=wire-probe,mc-test-harness,protocol-only bot,headless mock client\n'
  } > "$run_dir/automation-driver.txt"

  cat > "$run_dir/observations.json" <<EOF
{
  "schema": "solaris.real_client_observations.v1",
  "client_gate": "prepared-owner-run",
  "quality_label": "stabilization",
  "result": "not-run",
  "scenarios": []
}
EOF

  touch "$run_dir/client.log" "$run_dir/server.log"
}

prepare_run_dir() {
  local timestamp pack_id status run_dir
  require_file "$MANIFEST"
  require_file "$(server_config_path)"

  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  pack_id="$(basename "$MANIFEST" .json)"
  run_dir="$RUN_ROOT/$timestamp-$pack_id"
  status="$(client_command_status || true)"
  mkdir -p "$run_dir"
  write_run_templates "$run_dir" "$status"
  printf '%s\n' "$run_dir"
}

validate_run_dir() {
  local run_dir observations
  run_dir="$1"
  require_dir "$run_dir"
  for required in manifest.json client.log server.log observations.json screenshots git.txt toolchain.txt automation-driver.txt; do
    if [[ "$required" == screenshots ]]; then
      require_dir "$run_dir/$required"
    else
      require_file "$run_dir/$required"
    fi
  done
  observations="$(tr -d '\n[:space:]' < "$run_dir/observations.json")"
  if [[ "$observations" != *'"client_gate":"agent-runreal-client"'* && "$observations" != *'"client_gate":"agent-run-real-client"'* ]]; then
    printf 'error: observations.json does not record an agent-run real-client gate\n' >&2
    exit 1
  fi
  if [[ "$observations" == *'"result":"not-run"'* || "$observations" == *'"result":"prepared"'* ]]; then
    printf 'error: observations.json is still a prepared/not-run template\n' >&2
    exit 1
  fi
  printf 'validated %s\n' "$run_dir"
}

if [[ "$MODE" == "check" ]]; then
  client_command_status
  exit $?
fi

if [[ "$MODE" == "validate-run" ]]; then
  validate_run_dir "$VALIDATE_RUN_DIR"
  exit 0
fi

if [[ "$MODE" == "prepare" ]]; then
  prepare_run_dir
  exit 0
fi

if [[ "$MODE" == "run" ]]; then
  client_command_status >/dev/null
  run_dir="$(prepare_run_dir)"
  server_config="$(server_config_path)"
  printf 'running real-client regression into %s\n' "$run_dir"

  (
    cd "$REPO_ROOT"
    cargo run --bin mc-server -- --config "$server_config"
  ) > "$run_dir/server.log" 2>&1 &
  server_pid="$!"

  cleanup() {
    if kill -0 "$server_pid" >/dev/null 2>&1; then
      kill "$server_pid" >/dev/null 2>&1 || true
      wait "$server_pid" >/dev/null 2>&1 || true
    fi
  }
  trap cleanup EXIT

  sleep "$SERVER_START_SECONDS"
  set +e
  timeout "$CLIENT_TIMEOUT_SECONDS" bash -lc "$CLIENT_COMMAND" > "$run_dir/client.log" 2>&1
  client_status="$?"
  set -e

  {
    printf 'client_exit_status=%s\n' "$client_status"
    printf 'client_timeout_seconds=%s\n' "$CLIENT_TIMEOUT_SECONDS"
  } >> "$run_dir/automation-driver.txt"

  printf 'run artifacts: %s\n' "$run_dir"
  printf 'fill observations.json with executed scenario results, then run --validate-run %s\n' "$run_dir"
  exit "$client_status"
fi
