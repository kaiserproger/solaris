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
SERVER_ADDR="${SOLARIS_REAL_CLIENT_SERVER_ADDR:-127.0.0.1:25565}"
AGENT_DRIVER="${SOLARIS_REAL_CLIENT_AGENT_DRIVER:-$REPO_ROOT/tools/real-client-agent-driver.py}"
AGENT_BRIDGE_URL="${SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL:-}"
AGENT_SECRET="${SOLARIS_REAL_CLIENT_AGENT_SECRET:-}"
AGENT_JAR="${SOLARIS_REAL_CLIENT_AGENT_JAR:-$REPO_ROOT/client-mod/solaris-client-agent/java-agent/build/libs/java-agent-0.1.0.jar}"
AGENT_PORT="${SOLARIS_REAL_CLIENT_AGENT_PORT:-39094}"
AGENT_SCENARIO="${SOLARIS_REAL_CLIENT_AGENT_SCENARIO:-m94-02b-rejected-block-resync}"
AGENT_START_SECONDS="${SOLARIS_REAL_CLIENT_AGENT_START_SECONDS:-4}"
SECOND_CLIENT_COMMAND="${SOLARIS_REAL_CLIENT_SECOND_COMMAND:-}"
SECOND_AGENT_SECRET="${SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET:-}"
SECOND_AGENT_PORT="${SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT:-39095}"
SECOND_AGENT_BRIDGE_URL="${SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL:-}"
SECOND_AGENT_START_SECONDS="${SOLARIS_REAL_CLIENT_SECOND_AGENT_START_SECONDS:-$AGENT_START_SECONDS}"
MODE="prepare"
VALIDATE_RUN_DIR=""

if [[ -z "$AGENT_BRIDGE_URL" && -n "$AGENT_SECRET" ]]; then
  AGENT_BRIDGE_URL="http://127.0.0.1:${AGENT_PORT}/rpc"
fi
if [[ -z "$SECOND_AGENT_BRIDGE_URL" && -n "$SECOND_AGENT_SECRET" ]]; then
  SECOND_AGENT_BRIDGE_URL="http://127.0.0.1:${SECOND_AGENT_PORT}/rpc"
fi

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
  SOLARIS_REAL_CLIENT_SERVER_ADDR
                                Server address passed to the in-client agent driver.
                                Defaults to 127.0.0.1:25565.
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS
                                Timeout for --run client command. Defaults to 180.
  SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL
                                Loopback JSON bridge URL inside the real client.
  SOLARIS_REAL_CLIENT_AGENT_SECRET
                                Per-run bridge secret. Required for agent-driver mode.
  SOLARIS_REAL_CLIENT_AGENT_JAR
                                Java agent jar injected through JDK_JAVA_OPTIONS.
  SOLARIS_REAL_CLIENT_AGENT_PORT
                                Java agent bridge port. Defaults to 39094.
  SOLARIS_REAL_CLIENT_AGENT_DRIVER
                                Driver path. Defaults to tools/real-client-agent-driver.py.
  SOLARIS_REAL_CLIENT_AGENT_SCENARIO
                                Scenario id. Defaults to m94-02b-rejected-block-resync.
  SOLARIS_REAL_CLIENT_SECOND_COMMAND
                                Optional second real-client command for two-client gates.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET
                                Separate per-run bridge secret for the second client.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT
                                Second Java agent bridge port. Defaults to 39095.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL
                                Optional explicit loopback bridge URL for the second client.

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

validate_second_client_config() {
  local command_lc
  if [[ -z "$SECOND_CLIENT_COMMAND" && -z "$SECOND_AGENT_SECRET" && -z "$SECOND_AGENT_BRIDGE_URL" ]]; then
    return 1
  fi
  if [[ -z "$SECOND_CLIENT_COMMAND" || -z "$SECOND_AGENT_SECRET" || -z "$SECOND_AGENT_BRIDGE_URL" ]]; then
    printf 'error: second real-client mode requires SOLARIS_REAL_CLIENT_SECOND_COMMAND and SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET plus bridge URL or port\n' >&2
    exit 1
  fi
  command_lc="${SECOND_CLIENT_COMMAND,,}"
  if [[ "$command_lc" == *wire-probe* || "$command_lc" == *mc-test-harness* || "$command_lc" == *mc_test_harness* || "$command_lc" == *protocol-only* || "$command_lc" == *mock* ]]; then
    printf 'error: second real-client command must not be a protocol bot or mock\n' >&2
    exit 1
  fi
  if [[ "$command_lc" != *prism* && "$command_lc" != *minecraft* && "$command_lc" != *launcher* && "$command_lc" != *java* ]]; then
    printf 'error: second real-client command does not look like a vanilla/PrismLauncher client command\n' >&2
    exit 1
  fi
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
    printf 'server_addr=%s\n' "$SERVER_ADDR"
    printf 'client_agent_driver=%s\n' "$AGENT_DRIVER"
    printf 'client_agent_jar=%s\n' "$AGENT_JAR"
    printf 'client_agent_port=%s\n' "$AGENT_PORT"
    printf 'client_agent_scenario=%s\n' "$AGENT_SCENARIO"
    if [[ -n "$AGENT_BRIDGE_URL" ]]; then
      printf 'client_agent_bridge_url=%s\n' "$AGENT_BRIDGE_URL"
    else
      printf 'client_agent_bridge_url=UNSET_PREPARED_OWNER_RUN\n'
    fi
    if [[ -n "$AGENT_SECRET" ]]; then
      printf 'client_agent_secret=SET_REDACTED\n'
    else
      printf 'client_agent_secret=UNSET_PREPARED_OWNER_RUN\n'
    fi
    if [[ -n "$SECOND_CLIENT_COMMAND" ]]; then
      printf 'second_client_command=redacted\n'
      printf 'second_client_command_sha256=%s\n' "$(printf '%s' "$SECOND_CLIENT_COMMAND" | sha256sum | cut -d ' ' -f 1)"
    else
      printf 'second_client_command=UNSET\n'
    fi
    printf 'second_client_agent_port=%s\n' "$SECOND_AGENT_PORT"
    if [[ -n "$SECOND_AGENT_BRIDGE_URL" ]]; then
      printf 'second_client_agent_bridge_url=%s\n' "$SECOND_AGENT_BRIDGE_URL"
    else
      printf 'second_client_agent_bridge_url=UNSET\n'
    fi
    if [[ -n "$SECOND_AGENT_SECRET" ]]; then
      printf 'second_client_agent_secret=SET_REDACTED\n'
    else
      printf 'second_client_agent_secret=UNSET\n'
    fi
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
  python3 - "$run_dir" <<'PY'
import json
import sys
from pathlib import Path

run_dir = Path(sys.argv[1])
manifest = json.loads((run_dir / "manifest.json").read_text())
observations = json.loads((run_dir / "observations.json").read_text())
screenshots_dir = run_dir / "screenshots"
screenshots_root = screenshots_dir.resolve()

required_screenshots = {
    scenario.get("id")
    for scenario in manifest.get("scenarios", [])
    if scenario.get("screenshots_required") is True
}
for scenario in observations.get("scenarios", []):
    scenario_id = scenario.get("id")
    if scenario_id not in required_screenshots or scenario.get("result") != "passed":
        continue
    screenshots = scenario.get("screenshots")
    if not isinstance(screenshots, list) or not screenshots:
        print(
            f"error: scenario {scenario_id} requires at least one screenshots/ artifact",
            file=sys.stderr,
        )
        sys.exit(1)
    for screenshot in screenshots:
        if not isinstance(screenshot, str) or not screenshot.startswith("screenshots/"):
            print(
                f"error: scenario {scenario_id} screenshot must live under screenshots/: {screenshot!r}",
                file=sys.stderr,
            )
            sys.exit(1)
        screenshot_path = (run_dir / screenshot).resolve()
        try:
            screenshot_path.relative_to(screenshots_root)
        except ValueError:
            print(
                f"error: scenario {scenario_id} screenshot escapes screenshots/: {screenshot!r}",
                file=sys.stderr,
            )
            sys.exit(1)
        if not screenshot_path.is_file():
            print(
                f"error: scenario {scenario_id} screenshot is missing on disk: {screenshot}",
                file=sys.stderr,
            )
            sys.exit(1)
PY
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

  server_pid=""
  client_pid=""
  second_client_pid=""
  driver_timeout_seconds=$((CLIENT_TIMEOUT_SECONDS + 15))
  server_restart_count=0
  agent_phase_status=0
  second_client_enabled=0

  start_server() {
    local phase="$1"
    printf 'server_start_phase=%s\n' "$phase" >> "$run_dir/automation-driver.txt"
    (
      cd "$REPO_ROOT"
      cargo run --bin mc-server -- --config "$server_config"
    ) >> "$run_dir/server.log" 2>&1 &
    server_pid="$!"
    printf 'server_pid_%s=%s\n' "$phase" "$server_pid" >> "$run_dir/automation-driver.txt"
    sleep "$SERVER_START_SECONDS"
  }

  stop_server_gracefully() {
    local phase status
    phase="$1"
    if [[ -z "${server_pid:-}" ]]; then
      return
    fi
    if kill -0 "$server_pid" >/dev/null 2>&1; then
      printf 'server_stop_phase=%s signal=INT\n' "$phase" >> "$run_dir/automation-driver.txt"
      kill -INT "$server_pid" >/dev/null 2>&1 || true
    fi
    set +e
    wait "$server_pid" >/dev/null 2>&1
    status="$?"
    set -e
    printf 'server_exit_phase=%s status=%s\n' "$phase" "$status" >> "$run_dir/automation-driver.txt"
    server_pid=""
  }

  run_agent_driver_phase() {
    local scenario phase_status
    scenario="$1"
    shift
    printf 'client_agent_phase_scenario=%s\n' "$scenario" >> "$run_dir/automation-driver.txt"
    set +e
    timeout "$driver_timeout_seconds" python3 "$AGENT_DRIVER" \
      --bridge-url "$AGENT_BRIDGE_URL" \
      --secret "$AGENT_SECRET" \
      --run-dir "$run_dir" \
      --scenario "$scenario" \
      --server-addr "$SERVER_ADDR" \
      --timeout-seconds "$CLIENT_TIMEOUT_SECONDS" \
      "$@" >> "$run_dir/automation-driver.txt" 2>&1
    phase_status="$?"
    set -e
    agent_phase_status="$phase_status"
    printf 'client_agent_phase_exit_status_%s=%s\n' "$scenario" "$phase_status" >> "$run_dir/automation-driver.txt"
    return 0
  }

  launch_agent_client() {
    local label command port secret log_file pid_var jdk_options
    label="$1"
    command="$2"
    port="$3"
    secret="$4"
    log_file="$5"
    pid_var="$6"
    if [[ -f "$AGENT_JAR" ]]; then
      jdk_options="${JDK_JAVA_OPTIONS:+$JDK_JAVA_OPTIONS }--add-modules jdk.httpserver -javaagent:$AGENT_JAR=port=$port,runDir=$run_dir"
      printf 'client_agent_jar_injected_%s=%s\n' "$label" "$AGENT_JAR" >> "$run_dir/automation-driver.txt"
    elif [[ -n "${SOLARIS_REAL_CLIENT_AGENT_JAR:-}" ]]; then
      printf 'error: missing configured SOLARIS_REAL_CLIENT_AGENT_JAR: %s\n' "$AGENT_JAR" >&2
      exit 1
    else
      jdk_options="${JDK_JAVA_OPTIONS:-}"
      printf 'client_agent_jar_injected_%s=MISSING_DEFAULT_EXTERNAL_BRIDGE_ONLY\n' "$label" >> "$run_dir/automation-driver.txt"
    fi
    SOLARIS_CLIENT_AGENT_SECRET="$secret" \
      SOLARIS_CLIENT_AGENT_PORT="$port" \
      SOLARIS_CLIENT_AGENT_RUN_DIR="$run_dir" \
      JDK_JAVA_OPTIONS="$jdk_options" \
      bash -lc "$command" > "$log_file" 2>&1 &
    printf -v "$pid_var" '%s' "$!"
    printf 'client_pid_%s=%s\n' "$label" "${!pid_var}" >> "$run_dir/automation-driver.txt"
  }

  cleanup() {
    if [[ -n "${second_client_pid:-}" ]] && kill -0 "$second_client_pid" >/dev/null 2>&1; then
      kill "$second_client_pid" >/dev/null 2>&1 || true
      wait "$second_client_pid" >/dev/null 2>&1 || true
    fi
    if [[ -n "${client_pid:-}" ]] && kill -0 "$client_pid" >/dev/null 2>&1; then
      kill "$client_pid" >/dev/null 2>&1 || true
      wait "$client_pid" >/dev/null 2>&1 || true
    fi
    if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" >/dev/null 2>&1; then
      kill "$server_pid" >/dev/null 2>&1 || true
      wait "$server_pid" >/dev/null 2>&1 || true
    fi
  }
  trap cleanup EXIT

  : > "$run_dir/server.log"
  start_server "initial"
  if [[ -n "$AGENT_BRIDGE_URL" || -n "$AGENT_SECRET" ]]; then
    if [[ -z "$AGENT_BRIDGE_URL" || -z "$AGENT_SECRET" ]]; then
      printf 'error: agent-driver mode requires SOLARIS_REAL_CLIENT_AGENT_SECRET plus SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL or SOLARIS_REAL_CLIENT_AGENT_PORT\n' >&2
      exit 1
    fi
    require_file "$AGENT_DRIVER"
    if validate_second_client_config; then
      second_client_enabled=1
      printf 'second_client_enabled=1\n' >> "$run_dir/automation-driver.txt"
    else
      printf 'second_client_enabled=0\n' >> "$run_dir/automation-driver.txt"
    fi

    launch_agent_client "primary" "$CLIENT_COMMAND" "$AGENT_PORT" "$AGENT_SECRET" "$run_dir/client.log" client_pid
    sleep "$AGENT_START_SECONDS"

    if [[ "$AGENT_SCENARIO" == "m94-06-save-restart-two-client-visibility" ]]; then
      run_agent_driver_phase "m94-06-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "m94-06-before-restart"
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "m94-06-after-restart"
        run_agent_driver_phase "m94-06-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
        if [[ "$second_client_enabled" -eq 1 ]]; then
          launch_agent_client "secondary" "$SECOND_CLIENT_COMMAND" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
          sleep "$SECOND_AGENT_START_SECONDS"
          run_agent_driver_phase "m94-06-two-client-live-visibility" \
            --append-observations \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret "$SECOND_AGENT_SECRET"
          printf 'client_agent_two_client_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
          run_agent_driver_phase "m94-06-two-client-shared-drop" \
            --append-observations \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret "$SECOND_AGENT_SECRET"
          printf 'client_agent_two_client_shared_drop_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
          run_agent_driver_phase "m94-06-two-client-shared-pickup" \
            --append-observations \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret "$SECOND_AGENT_SECRET"
          printf 'client_agent_two_client_shared_pickup_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
          driver_status=1
        fi
      fi
    elif [[ "$AGENT_SCENARIO" == "m94-03b-two-client-shared-chest" || "$AGENT_SCENARIO" == "m94-03c-two-client-shared-chest-live-update" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_agent_client "secondary" "$SECOND_CLIENT_COMMAND" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        sleep "$SECOND_AGENT_START_SECONDS"
        run_agent_driver_phase "$AGENT_SCENARIO" \
          --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
          --secondary-secret "$SECOND_AGENT_SECRET"
        if [[ "$AGENT_SCENARIO" == "m94-03c-two-client-shared-chest-live-update" ]]; then
          printf 'client_agent_two_client_shared_chest_live_update_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        else
          printf 'client_agent_two_client_shared_chest_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        fi
        driver_status="$agent_phase_status"
      else
        run_agent_driver_phase "$AGENT_SCENARIO"
        driver_status="$agent_phase_status"
      fi
    else
      run_agent_driver_phase "$AGENT_SCENARIO"
      driver_status="$agent_phase_status"
    fi

    set +e
    second_client_status=0
    if [[ -n "${second_client_pid:-}" ]]; then
      if kill -0 "$second_client_pid" >/dev/null 2>&1; then
        kill "$second_client_pid" >/dev/null 2>&1 || true
        wait "$second_client_pid" >/dev/null 2>&1
        second_client_status="$?"
      else
        wait "$second_client_pid" >/dev/null 2>&1
        second_client_status="$?"
      fi
    fi
    if kill -0 "$client_pid" >/dev/null 2>&1; then
      kill "$client_pid" >/dev/null 2>&1 || true
      wait "$client_pid" >/dev/null 2>&1
      client_status="$?"
    else
      wait "$client_pid" >/dev/null 2>&1
      client_status="$?"
    fi
    set -e

    {
      printf 'client_exit_status=%s\n' "$client_status"
      printf 'client_timeout_seconds=%s\n' "$CLIENT_TIMEOUT_SECONDS"
      printf 'second_client_exit_status=%s\n' "$second_client_status"
      printf 'client_agent_driver_timeout_seconds=%s\n' "$driver_timeout_seconds"
      printf 'client_agent_driver_exit_status=%s\n' "$driver_status"
      printf 'server_restart_count=%s\n' "$server_restart_count"
    } >> "$run_dir/automation-driver.txt"

    printf 'run artifacts: %s\n' "$run_dir"
    printf 'validate with: bash tools/run-real-client-regression.sh --validate-run %s\n' "$run_dir"
    exit "$driver_status"
  fi

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
