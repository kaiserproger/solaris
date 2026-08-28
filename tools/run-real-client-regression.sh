#!/usr/bin/env bash
# Prepare or run the M94+ real-client regression pack.
# This path is intentionally fail-closed: protocol bots and mocks are rejected,
# and prepared artifacts do not count as passed client evidence.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="${SOLARIS_REAL_CLIENT_MANIFEST:-$REPO_ROOT/docs/real-client-regression/manifests/m94-regression-pack.json}"
RUN_ROOT="${SOLARIS_REAL_CLIENT_RUN_ROOT:-$REPO_ROOT/.analysis/real-client-runs}"
AGENT_ROOT="$REPO_ROOT/client-mod/solaris-client-agent"
GRADLE_RUNCLIENT_TASK=":fabric-agent:runClientAgent"
CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
CLIENT_KIND="gradle-runclient"
SECOND_CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
SECOND_CLIENT_KIND="gradle-runclient"
CLIENT_TIMEOUT_SECONDS="${SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS:-180}"
SERVER_READY_TIMEOUT_SECONDS="${SOLARIS_REAL_CLIENT_SERVER_READY_TIMEOUT_SECONDS:-120}"
SERVER_CONFIG="${SOLARIS_REAL_CLIENT_SERVER_CONFIG:-example.toml}"
FRESH_WORLD="${SOLARIS_REAL_CLIENT_FRESH_WORLD:-0}"
SERVER_SEED="${SOLARIS_REAL_CLIENT_SERVER_SEED:-}"
SERVER_ADDR="${SOLARIS_REAL_CLIENT_SERVER_ADDR:-127.0.0.1:25565}"
AGENT_DRIVER="${SOLARIS_REAL_CLIENT_AGENT_DRIVER:-$REPO_ROOT/tools/real-client-agent-driver.py}"
AGENT_BRIDGE_URL="${SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL:-}"
AGENT_SECRET="${SOLARIS_REAL_CLIENT_AGENT_SECRET:-}"
AGENT_PORT="${SOLARIS_REAL_CLIENT_AGENT_PORT:-}"
AGENT_SCENARIO="${SOLARIS_REAL_CLIENT_AGENT_SCENARIO:-m94-02b-rejected-block-resync}"
SECOND_AGENT_SECRET="${SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET:-}"
SECOND_AGENT_PORT="${SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT:-}"
SECOND_AGENT_BRIDGE_URL="${SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL:-}"
MODE="prepare"
VALIDATE_RUN_DIR=""
SCENARIO_NO_DEBUG="0"

usage() {
  cat <<'EOF'
Usage: tools/run-real-client-regression.sh [--check|--prepare|--run|--validate-run DIR]

Environment:
	  Primary client adapter is auto-selected as the repo-native
	                                Gradle runClient adapter:
	                                client-mod/solaris-client-agent/gradlew
	                                --no-configuration-cache :fabric-agent:runClientAgent.
  SOLARIS_REAL_CLIENT_MANIFEST  Regression pack manifest path.
  SOLARIS_REAL_CLIENT_RUN_ROOT  Local-only artifact root. Defaults to .analysis/real-client-runs.
  SOLARIS_REAL_CLIENT_SERVER_CONFIG
                                Server config path. Defaults to example.toml.
  SOLARIS_REAL_CLIENT_FRESH_WORLD
                                If 1, copy the server config into the run dir
                                and point data.world_dir at a fresh per-run world.
                                --run always uses a per-run server.toml that
                                grants the Gradle client usernames operator
                                access for local debug commands, except
                                playable-46-generated-ruin-cache which uses
                                no operators.
  SOLARIS_REAL_CLIENT_SERVER_SEED
                                Optional signed decimal integer used to override
                                data.seed in the prepared server config.
  SOLARIS_REAL_CLIENT_SERVER_ADDR
                                Server address passed to the in-client agent driver.
                                Defaults to 127.0.0.1:25565.
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS
                                Timeout for the Gradle runClient adapter.
                                Defaults to 180.
  SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL
                                Loopback JSON bridge URL inside the real client.
  SOLARIS_REAL_CLIENT_AGENT_SECRET
                                Per-run bridge secret. Required for agent-driver mode.
  SOLARIS_REAL_CLIENT_AGENT_PORT
                                Java agent bridge port. Defaults to a free
                                loopback port selected per run.
  SOLARIS_REAL_CLIENT_AGENT_DRIVER
                                Driver path. Defaults to tools/real-client-agent-driver.py.
  SOLARIS_REAL_CLIENT_AGENT_SCENARIO
                                Scenario id. Defaults to m94-02b-rejected-block-resync.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET
                                Separate per-run bridge secret for the second client.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT
                                Second Java agent bridge port. Defaults to a
                                free loopback port selected per run.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL
                                Optional explicit loopback bridge URL for the second client.

Modes:
  --check         Validate that the Gradle runClient adapter is available.
  --prepare       Create a local run directory and observation templates.
  --run           Start Solaris, launch Gradle runClient, and record logs.
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

if [[ -n "$SERVER_SEED" ]]; then
  if [[ ! "$SERVER_SEED" =~ ^[+-]?[0-9]+$ ]] \
    || ! SERVER_SEED="$(python3 - "$SERVER_SEED" <<'PY'
import sys

value = int(sys.argv[1], 10)
if value < -(1 << 63) or value > (1 << 63) - 1:
    raise SystemExit(1)
print(value)
PY
)"; then
    printf 'error: SOLARIS_REAL_CLIENT_SERVER_SEED must be a signed 64-bit decimal integer\n' >&2
    exit 2
  fi
fi

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

manifest_quality_label() {
  python3 - "$MANIFEST" <<'PY'
import json
import sys
from pathlib import Path

manifest = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
quality_label = manifest.get("quality_label")
if not isinstance(quality_label, str) or not quality_label:
    print("error: manifest quality_label must be a non-empty string", file=sys.stderr)
    sys.exit(1)
print(quality_label)
PY
}

requested_scenario_no_debug() {
  require_file "$MANIFEST"
  python3 - "$MANIFEST" "$AGENT_SCENARIO" <<'PY'
import json
import sys
from pathlib import Path

manifest_path = Path(sys.argv[1])
scenario_id = sys.argv[2]
manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
scenarios = manifest.get("scenarios")
if not isinstance(scenarios, list):
    print("error: manifest scenarios must be a list", file=sys.stderr)
    sys.exit(1)
matches = [
    scenario
    for scenario in scenarios
    if isinstance(scenario, dict) and scenario.get("id") == scenario_id
]
if not matches:
    print(
        f"error: requested scenario {scenario_id!r} is missing from manifest {manifest_path}",
        file=sys.stderr,
    )
    sys.exit(1)
if len(matches) != 1:
    print(
        f"error: requested scenario {scenario_id!r} is duplicated in manifest {manifest_path}",
        file=sys.stderr,
    )
    sys.exit(1)
no_debug = matches[0].get("no_debug_commands")
if not isinstance(no_debug, bool):
    print(
        f"error: requested scenario {scenario_id!r} must declare boolean no_debug_commands",
        file=sys.stderr,
    )
    sys.exit(1)
print("1" if no_debug else "0")
PY
}

manifest_core_replay_source() {
  local manifest_path
  manifest_path="${1:-$MANIFEST}"
  python3 - "$manifest_path" <<'PY'
import json
import sys
from pathlib import Path, PurePosixPath

manifest = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
value = manifest.get("core_replay_manifest")
if value is None:
    sys.exit(0)
if not isinstance(value, str) or not value:
    print("error: core_replay_manifest must be a non-empty relative path", file=sys.stderr)
    sys.exit(1)
path = PurePosixPath(value)
if (
    path.is_absolute()
    or value != path.as_posix()
    or any(part in {"", ".", ".."} for part in path.parts)
    or path.parts[:2] != ("tools", "core-replay-scenarios")
    or path.suffix != ".json"
):
    print(
        "error: core_replay_manifest must name a JSON file under tools/core-replay-scenarios",
        file=sys.stderr,
    )
    sys.exit(1)
print(value)
PY
}

pick_free_loopback_port() {
  python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

bridge_port_from_url() {
  python3 - "$1" <<'PY'
import sys
from urllib.parse import urlparse

parsed = urlparse(sys.argv[1])
if parsed.port is None:
    sys.exit(1)
print(parsed.port)
PY
}

configure_primary_agent_bridge() {
  if [[ -n "$AGENT_BRIDGE_URL" ]]; then
    if [[ -z "$AGENT_PORT" ]]; then
      if ! AGENT_PORT="$(bridge_port_from_url "$AGENT_BRIDGE_URL")"; then
        printf 'error: SOLARIS_REAL_CLIENT_AGENT_PORT is required when SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL has no explicit port\n' >&2
        exit 1
      fi
    fi
    return 0
  fi
  if [[ -n "$AGENT_SECRET" ]]; then
    if [[ -z "$AGENT_PORT" ]]; then
      AGENT_PORT="$(pick_free_loopback_port)"
    fi
    AGENT_BRIDGE_URL="http://127.0.0.1:${AGENT_PORT}/rpc"
  fi
}

configure_second_agent_bridge() {
  if [[ -z "$SECOND_AGENT_SECRET" && -z "$SECOND_AGENT_BRIDGE_URL" && -z "$SECOND_AGENT_PORT" ]]; then
    return 0
  fi
  if [[ -n "$SECOND_AGENT_BRIDGE_URL" ]]; then
    if [[ -z "$SECOND_AGENT_PORT" ]]; then
      if ! SECOND_AGENT_PORT="$(bridge_port_from_url "$SECOND_AGENT_BRIDGE_URL")"; then
        printf 'error: SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT is required when SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL has no explicit port\n' >&2
        exit 1
      fi
    fi
    return 0
  fi
  if [[ -n "$SECOND_AGENT_SECRET" ]]; then
    if [[ -z "$SECOND_AGENT_PORT" ]]; then
      SECOND_AGENT_PORT="$(pick_free_loopback_port)"
      while [[ -n "$AGENT_PORT" && "$SECOND_AGENT_PORT" == "$AGENT_PORT" ]]; do
        SECOND_AGENT_PORT="$(pick_free_loopback_port)"
      done
    fi
    SECOND_AGENT_BRIDGE_URL="http://127.0.0.1:${SECOND_AGENT_PORT}/rpc"
  fi
}

gradle_runclient_adapter_available() {
  local build_file
  build_file="$AGENT_ROOT/fabric-agent/build.gradle.kts"

  [[ -x "$AGENT_ROOT/gradlew" && -f "$build_file" ]] || return 1
  grep -Fq 'net.neoforged.moddev' "$build_file" || return 1
  grep -Fq 'create("clientAgent")' "$build_file" || return 1
  grep -Fq 'tasks.named("runClientAgent")' "$build_file" || return 1
  grep -Fq 'validateClientAgentRunProperties' "$build_file" || return 1
  grep -Fq 'gameDirectory.set(file(clientAgentGameDir.get()))' "$build_file" || return 1
  grep -Fq 'programArgument("--username")' "$build_file" || return 1
  grep -Fq 'programArgument(clientAgentUsername.get())' "$build_file"
}

resolve_client_adapter() {
  if gradle_runclient_adapter_available; then
    CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
    CLIENT_KIND="gradle-runclient"
    return 0
  fi

  CLIENT_ADAPTER_SOURCE="missing"
  return 1
}

server_config_path() {
  if [[ "$SERVER_CONFIG" = /* ]]; then
    printf '%s\n' "$SERVER_CONFIG"
  else
    printf '%s/%s\n' "$REPO_ROOT" "$SERVER_CONFIG"
  fi
}

client_username_for_label() {
  case "$1" in
    primary)
      printf 'SolarisPrimary\n'
      ;;
    secondary)
      printf 'SolarisSecondary\n'
      ;;
    *)
      printf 'SolarisClient\n'
      ;;
  esac
}

client_game_dir_for_label() {
  printf '%s/clients/%s\n' "$run_dir" "$1"
}

real_client_operator_list() {
  if [[ "$SCENARIO_NO_DEBUG" == "1" ]]; then
    printf '[]\n'
    return
  fi
  printf '["%s", "%s"]\n' "$(client_username_for_label primary)" "$(client_username_for_label secondary)"
}

write_real_client_server_config() {
  local source_config target_config world_dir operators hostile_spawn_interval_override
  source_config="$1"
  target_config="$2"
  world_dir="$3"
  operators="$(real_client_operator_list)"
  case "$AGENT_SCENARIO" in
    playable-04-twenty-minute-survival-loop)
      hostile_spawn_interval_override=""
      ;;
    playable-*)
      hostile_spawn_interval_override="20"
      ;;
    *)
      hostile_spawn_interval_override=""
      ;;
  esac
  if [[ -n "$world_dir" && "$MODE" == "run" ]]; then
    mkdir -p "$world_dir"
  fi
  awk -v world_dir="$world_dir" -v operators="$operators" -v hostile_spawn_interval_override="$hostile_spawn_interval_override" -v seed_override="$SERVER_SEED" '
    BEGIN {
      section = ""
      seen_admin = 0
      seen_simulation = 0
      wrote_admin_operators = 0
      wrote_hostile_spawn_interval = 0
      replaced_world_dir = 0
      replaced_seed = 0
      escaped_world_dir = world_dir
      gsub(/\\/, "\\\\", escaped_world_dir)
      gsub(/"/, "\\\"", escaped_world_dir)
    }
    function emit_admin_operators_if_needed() {
      if (section == "admin" && wrote_admin_operators == 0) {
        print "operators = " operators
        wrote_admin_operators = 1
      }
    }
    function emit_hostile_spawn_interval_if_needed() {
      if (section == "simulation" && hostile_spawn_interval_override != "" && wrote_hostile_spawn_interval == 0) {
        print "hostile_spawn_interval_ticks = " hostile_spawn_interval_override
        wrote_hostile_spawn_interval = 1
      }
    }
    /^[[:space:]]*\[[^]]+\][[:space:]]*$/ {
      emit_admin_operators_if_needed()
      emit_hostile_spawn_interval_if_needed()
      section = $0
      gsub(/^[[:space:]]*\[/, "", section)
      gsub(/\][[:space:]]*$/, "", section)
      if (section == "admin") {
        seen_admin = 1
      }
      if (section == "simulation") {
        seen_simulation = 1
      }
      print
      next
    }
    section == "data" && world_dir != "" && /^[[:space:]]*world_dir[[:space:]]*=/ && replaced_world_dir == 0 {
      print "world_dir = \"" escaped_world_dir "\""
      replaced_world_dir = 1
      next
    }
    section == "data" && seed_override != "" && /^[[:space:]]*seed[[:space:]]*=/ && replaced_seed == 0 {
      print "seed = " seed_override
      replaced_seed = 1
      next
    }
    section == "admin" && /^[[:space:]]*operators[[:space:]]*=/ {
      print "operators = " operators
      wrote_admin_operators = 1
      next
    }
    section == "simulation" && hostile_spawn_interval_override != "" && /^[[:space:]]*hostile_spawn_interval_ticks[[:space:]]*=/ {
      print "hostile_spawn_interval_ticks = " hostile_spawn_interval_override
      wrote_hostile_spawn_interval = 1
      next
    }
    { print }
    END {
      emit_admin_operators_if_needed()
      emit_hostile_spawn_interval_if_needed()
      if (seen_admin == 0) {
        print ""
        print "[admin]"
        print "operators = " operators
      }
      if (world_dir != "" && replaced_world_dir == 0) {
        print "error: server config has no data.world_dir setting" > "/dev/stderr"
        exit 1
      }
      if (seed_override != "" && replaced_seed == 0) {
        print "error: server config has no data.seed setting" > "/dev/stderr"
        exit 1
      }
      if (hostile_spawn_interval_override != "" && seen_simulation == 0) {
        print ""
        print "[simulation]"
        print "hostile_spawn_interval_ticks = " hostile_spawn_interval_override
      }
    }
  ' "$source_config" > "$target_config"
}

client_adapter_status() {
  if ! resolve_client_adapter; then
    printf 'degraded: auto-discovery found no supported real-client launcher; repo-native Gradle runClient adapter is missing\n'
    return 1
  fi

  printf 'agent-run real-client configured: gradle-runclient\n'
  return 0
}

validate_second_client_config() {
  if [[ -z "$SECOND_AGENT_SECRET" && -z "$SECOND_AGENT_BRIDGE_URL" ]]; then
    return 1
  fi
  if [[ -z "$SECOND_AGENT_SECRET" || -z "$SECOND_AGENT_BRIDGE_URL" ]]; then
    printf 'error: second real-client mode requires SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET plus bridge URL or port\n' >&2
    exit 1
  fi
  if ! gradle_runclient_adapter_available; then
    printf 'error: second real-client mode requires the repo-native Gradle runClient adapter\n' >&2
    exit 1
  fi
  SECOND_CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
  SECOND_CLIENT_KIND="gradle-runclient"
  return 0
}

write_run_templates() {
  local run_dir status manifest_sha quality_label core_replay_source core_replay_sha
  run_dir="$1"
  status="$2"
  manifest_sha="$(sha256sum "$MANIFEST" | cut -d ' ' -f 1)"
  quality_label="$(manifest_quality_label)"
  core_replay_source="$(manifest_core_replay_source)"

  mkdir -p "$run_dir/screenshots"
  cp "$MANIFEST" "$run_dir/manifest.json"
  if [[ -n "$core_replay_source" ]]; then
    require_file "$REPO_ROOT/$core_replay_source"
    cp "$REPO_ROOT/$core_replay_source" "$run_dir/core-replay-manifest.json"
    core_replay_sha="$(sha256sum "$run_dir/core-replay-manifest.json" | cut -d ' ' -f 1)"
  else
    core_replay_sha=""
  fi

  {
    printf 'branch=%s\n' "$(git -C "$REPO_ROOT" branch --show-current)"
    printf 'commit=%s\n' "$(git -C "$REPO_ROOT" rev-parse HEAD)"
    printf 'status_short_begin\n'
    git -C "$REPO_ROOT" status --short --branch
    printf 'status_short_end\n'
    printf 'manifest=%s\n' "$MANIFEST"
    printf 'manifest_sha256=%s\n' "$manifest_sha"
    if [[ -n "$core_replay_source" ]]; then
      printf 'core_replay_source=%s\n' "$core_replay_source"
      printf 'core_replay_sha256=%s\n' "$core_replay_sha"
    fi
  } > "$run_dir/git.txt"

  {
    printf 'rustc=%s\n' "$(rustc --version)"
    printf 'cargo=%s\n' "$(cargo --version)"
    printf 'java=%s\n' "$(java --version 2>&1 | tr '\n' ' ')"
  } > "$run_dir/toolchain.txt"

  {
    printf 'client_gate=%s\n' "$status"
    printf 'client_kind=%s\n' "${CLIENT_KIND:-UNSET}"
    printf 'client_adapter_source=%s\n' "$CLIENT_ADAPTER_SOURCE"
    printf 'client_adapter_root=%s\n' "$AGENT_ROOT"
    printf 'client_adapter_gradlew=%s\n' "$AGENT_ROOT/gradlew"
    printf 'client_adapter_task=%s\n' "$GRADLE_RUNCLIENT_TASK"
    printf 'client_adapter_args=%s\n' "--no-configuration-cache $GRADLE_RUNCLIENT_TASK"
    printf 'client_adapter_sha256=%s\n' "$(printf '%s' "$AGENT_ROOT/gradlew --no-configuration-cache $GRADLE_RUNCLIENT_TASK" | sha256sum | cut -d ' ' -f 1)"
    printf 'client_username=%s\n' "$(client_username_for_label primary)"
    printf 'client_game_dir=%s\n' "$(client_game_dir_for_label primary)"
    printf 'server_command=%s\n' "cargo run --bin mc-server -- --config $(server_config_path)"
    printf 'server_addr=%s\n' "$SERVER_ADDR"
    printf 'client_agent_driver=%s\n' "$AGENT_DRIVER"
    printf 'client_agent_runtime_provenance_primary=PENDING_GRADLE_RUNCLIENT\n'
    printf 'client_agent_port=%s\n' "$AGENT_PORT"
    printf 'client_agent_scenario=%s\n' "$AGENT_SCENARIO"
    if [[ -n "$core_replay_source" ]]; then
      printf 'core_replay_manifest=core-replay-manifest.json\n'
      printf 'core_replay_manifest_sha256=%s\n' "$core_replay_sha"
    fi
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
    printf 'second_client_kind=%s\n' "${SECOND_CLIENT_KIND:-UNSET}"
    printf 'second_client_adapter_source=%s\n' "$SECOND_CLIENT_ADAPTER_SOURCE"
    printf 'second_client_adapter_root=%s\n' "$AGENT_ROOT"
    printf 'second_client_adapter_gradlew=%s\n' "$AGENT_ROOT/gradlew"
    printf 'second_client_adapter_task=%s\n' "$GRADLE_RUNCLIENT_TASK"
    printf 'second_client_adapter_args=%s\n' "--no-configuration-cache $GRADLE_RUNCLIENT_TASK"
    printf 'second_client_adapter_sha256=%s\n' "$(printf '%s' "$AGENT_ROOT/gradlew --no-configuration-cache $GRADLE_RUNCLIENT_TASK" | sha256sum | cut -d ' ' -f 1)"
    printf 'second_client_username=%s\n' "$(client_username_for_label secondary)"
    printf 'second_client_game_dir=%s\n' "$(client_game_dir_for_label secondary)"
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
  "quality_label": "$quality_label",
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
  resolve_client_adapter || true

  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  pack_id="$(basename "$MANIFEST" .json)"
  mkdir -p "$RUN_ROOT"
  run_dir="$(mktemp -d "$RUN_ROOT/$timestamp-$pack_id-XXXXXX")"
  status="$(client_adapter_status || true)"
  write_run_templates "$run_dir" "$status"
  printf '%s\n' "$run_dir"
}

validate_run_dir() {
  local run_dir observations core_replay_source
  run_dir="$1"
  require_dir "$run_dir"
  for required in manifest.json client.log server.log observations.json screenshots git.txt toolchain.txt automation-driver.txt; do
    if [[ "$required" == screenshots ]]; then
      require_dir "$run_dir/$required"
    else
      require_file "$run_dir/$required"
    fi
  done
  core_replay_source="$(manifest_core_replay_source "$run_dir/manifest.json")"
  if [[ -n "$core_replay_source" ]]; then
    require_file "$run_dir/core-replay-manifest.json"
    require_file "$run_dir/core-replay-result.json"
    (
      cd "$REPO_ROOT"
      cargo run --quiet -p mc-test-harness --bin core-replay-validate -- \
        "$run_dir/core-replay-manifest.json" \
        "$run_dir/core-replay-result.json"
    )
  fi
  observations="$(tr -d '\n[:space:]' < "$run_dir/observations.json")"
  if [[ "$observations" != *'"client_gate":"agent-run-real-client"'* ]]; then
    printf 'error: observations.json client_gate must be agent-run-real-client\n' >&2
    exit 1
  fi
  if [[ "$observations" == *'"result":"not-run"'* || "$observations" == *'"result":"prepared"'* ]]; then
    printf 'error: observations.json is still a prepared/not-run template\n' >&2
    exit 1
  fi
  validate_automation_driver "$run_dir/automation-driver.txt"
  validate_automation_driver_for_observations "$run_dir/automation-driver.txt" "$observations" "$run_dir/observations.json" "$run_dir/manifest.json"
  validate_server_log "$run_dir/server.log" "$observations"
  python3 - "$run_dir" <<'PY'
import binascii
import json
import sys
from pathlib import Path

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
EXPECTED_OBSERVATIONS_SCHEMA = "solaris.real_client_observations.v1"
MIN_SCREENSHOT_WIDTH = 320
MIN_SCREENSHOT_HEIGHT = 180


def png_validation_error(path):
    try:
        data = path.read_bytes()
    except OSError as exc:
        return f"could not read file: {exc}"

    if not data.startswith(PNG_SIGNATURE):
        return "missing PNG signature"
    offset = len(PNG_SIGNATURE)
    saw_ihdr = False
    while True:
        if offset + 12 > len(data):
            return "truncated PNG chunk header"
        length = int.from_bytes(data[offset : offset + 4], "big")
        chunk_type = data[offset + 4 : offset + 8]
        chunk_start = offset + 8
        chunk_end = chunk_start + length
        crc_end = chunk_end + 4
        if crc_end > len(data):
            return f"truncated {chunk_type.decode('ascii', errors='replace')} chunk"
        expected_crc = int.from_bytes(data[chunk_end:crc_end], "big")
        actual_crc = binascii.crc32(chunk_type + data[chunk_start:chunk_end]) & 0xFFFFFFFF
        if actual_crc != expected_crc:
            return f"{chunk_type.decode('ascii', errors='replace')} CRC mismatch"
        if not saw_ihdr:
            if chunk_type != b"IHDR":
                return "first PNG chunk is not IHDR"
            if length != 13:
                return "IHDR chunk has invalid length"
            width = int.from_bytes(data[chunk_start : chunk_start + 4], "big")
            height = int.from_bytes(data[chunk_start + 4 : chunk_start + 8], "big")
            if width < MIN_SCREENSHOT_WIDTH or height < MIN_SCREENSHOT_HEIGHT:
                return (
                    f"screenshot size {width}x{height} is smaller than "
                    f"{MIN_SCREENSHOT_WIDTH}x{MIN_SCREENSHOT_HEIGHT}"
                )
            saw_ihdr = True
        if chunk_type == b"IEND":
            if length != 0:
                return "IEND chunk has invalid length"
            if crc_end != len(data):
                return "trailing bytes after IEND"
            return None
        offset = crc_end


run_dir = Path(sys.argv[1])
manifest = json.loads((run_dir / "manifest.json").read_text())
observations = json.loads((run_dir / "observations.json").read_text())
if not isinstance(observations, dict):
    print("error: observations.json must be a JSON object", file=sys.stderr)
    sys.exit(1)
if observations.get("schema") != EXPECTED_OBSERVATIONS_SCHEMA:
    print(
        f"error: observations.json schema must be {EXPECTED_OBSERVATIONS_SCHEMA}",
        file=sys.stderr,
    )
    sys.exit(1)
manifest_quality_label = manifest.get("quality_label")
if not isinstance(manifest_quality_label, str) or not manifest_quality_label:
    print(
        "error: manifest.json quality_label must be a non-empty string",
        file=sys.stderr,
    )
    sys.exit(1)
if observations.get("quality_label") != manifest_quality_label:
    print(
        f"error: observations.json quality_label must match manifest quality_label {manifest_quality_label}",
        file=sys.stderr,
    )
    sys.exit(1)
if observations.get("result") != "passed":
    print(
        f"error: observations.json result must be passed, got {observations.get('result')!r}",
        file=sys.stderr,
    )
    sys.exit(1)
observed_scenarios = observations.get("scenarios")
if not isinstance(observed_scenarios, list) or not observed_scenarios:
    print(
        "error: observations.json scenarios must contain at least one passed scenario",
        file=sys.stderr,
    )
    sys.exit(1)
screenshots_dir = run_dir / "screenshots"
screenshots_root = screenshots_dir.resolve()

required_screenshots = {
    scenario.get("id")
    for scenario in manifest.get("scenarios", [])
    if scenario.get("screenshots_required") is True
}
manifest_scenarios = {
    scenario.get("id")
    for scenario in manifest.get("scenarios", [])
    if isinstance(scenario.get("id"), str)
}
for scenario in observed_scenarios:
    scenario_id = scenario.get("id")
    if not isinstance(scenario_id, str) or scenario_id not in manifest_scenarios:
        print(
            f"error: observations.json contains unknown scenario id: {scenario_id!r}",
            file=sys.stderr,
        )
        sys.exit(1)
    if scenario.get("result") != "passed":
        print(
            f"error: scenario {scenario_id} result must be passed, got {scenario.get('result')!r}",
            file=sys.stderr,
        )
        sys.exit(1)
    screenshots = scenario.get("screenshots")
    if scenario_id in required_screenshots and (not isinstance(screenshots, list) or not screenshots):
        print(
            f"error: scenario {scenario_id} requires at least one screenshots/ artifact",
            file=sys.stderr,
        )
        sys.exit(1)
    if screenshots is None:
        continue
    if not isinstance(screenshots, list):
        print(
            f"error: scenario {scenario_id} screenshots must be a list",
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
        png_error = png_validation_error(screenshot_path)
        if png_error is not None:
            print(
                f"error: scenario {scenario_id} screenshot is invalid PNG: {screenshot}: {png_error}",
                file=sys.stderr,
            )
            sys.exit(1)
PY
  printf 'validated %s\n' "$run_dir"
}

validate_automation_driver() {
  local automation_driver legacy_matches legacy_primary_env legacy_m94_env bad_status_matches bad_bridge_matches phase_status_count
  automation_driver="$1"
  legacy_primary_env="$(printf '%s_%s_%s_%s' SOLARIS REAL CLIENT COMMAND)"
  legacy_m94_env="$(printf '%s_%s_%s' M94 CLIENT COMMAND)"

  if ! grep -Fxq 'client_kind=gradle-runclient' "$automation_driver"; then
    printf 'error: automation-driver.txt must record client_kind=gradle-runclient\n' >&2
    exit 1
  fi
  if ! grep -Fxq 'client_adapter_source=auto-gradle-runclient' "$automation_driver"; then
    printf 'error: automation-driver.txt must record client_adapter_source=auto-gradle-runclient\n' >&2
    exit 1
  fi
  if ! grep -Fxq 'client_adapter_task=:fabric-agent:runClientAgent' "$automation_driver"; then
    printf 'error: automation-driver.txt must record client_adapter_task=:fabric-agent:runClientAgent\n' >&2
    exit 1
  fi
  if grep -Eq '^client_agent_jar(_injected)?(_(primary|secondary))?=' "$automation_driver"; then
    printf 'error: automation-driver.txt must not claim an irrelevant java-agent jar\n' >&2
    exit 1
  fi

  validate_runtime_provenance() {
    local label
    label="$1"
    if ! grep -Fxq "client_agent_runtime_kind_${label}=compiled-classes" "$automation_driver"; then
      printf 'error: automation-driver.txt must record %s runtime classes provenance\n' "$label" >&2
      exit 1
    fi
    if ! grep -Eq "^client_agent_runtime_path_${label}=.+/fabric-agent/build/classes/java/main$" "$automation_driver"; then
      printf 'error: automation-driver.txt must record the %s Gradle runtime classes path\n' "$label" >&2
      exit 1
    fi
    if ! grep -Eq "^client_agent_runtime_sha256_${label}=[0-9a-f]{64}$" "$automation_driver"; then
      printf 'error: automation-driver.txt must record the %s runtime classes digest\n' "$label" >&2
      exit 1
    fi
    if ! grep -Eq "^client_agent_runtime_file_count_${label}=[1-9][0-9]*$" "$automation_driver"; then
      printf 'error: automation-driver.txt must record the %s runtime classes file count\n' "$label" >&2
      exit 1
    fi
    if ! grep -Fxq "client_agent_runtime_validation_${label}=verified" "$automation_driver"; then
      printf 'error: automation-driver.txt must record verified %s runtime classes provenance\n' "$label" >&2
      exit 1
    fi
  }
  validate_runtime_provenance primary

  legacy_matches="$(
    grep -En \
      "^(client_command|second_client_command)=|${legacy_primary_env}|${legacy_m94_env}|client_adapter_source=legacy-command|client_kind=(prismlauncher|command)" \
      "$automation_driver" || true
  )"
  if [[ -n "$legacy_matches" ]]; then
    printf 'error: automation-driver.txt contains legacy client-command adapter metadata\n' >&2
    sed -n '1,20p' <<< "$legacy_matches" >&2
    exit 1
  fi

  if ! grep -Fxq 'client_agent_driver_exit_status=0' "$automation_driver"; then
    printf 'error: automation-driver.txt must record client_agent_driver_exit_status=0\n' >&2
    exit 1
  fi
  phase_status_count="$(grep -Ec '^client_agent_phase_exit_status_[^=]+=0$' "$automation_driver" || true)"
  if [[ "$phase_status_count" -lt 1 ]]; then
    printf 'error: automation-driver.txt must record at least one client-agent phase exit status 0\n' >&2
    exit 1
  fi
  bad_status_matches="$(
    awk -F= '/^client_agent_.*exit_status/ && $2 != "0" { print NR ":" $0 }' "$automation_driver" || true
  )"
  if [[ -n "$bad_status_matches" ]]; then
    printf 'error: automation-driver.txt contains nonzero client-agent exit status\n' >&2
    sed -n '1,20p' <<< "$bad_status_matches" >&2
    exit 1
  fi
  if ! grep -Fxq 'client_agent_bridge_wait_status_primary=ready' "$automation_driver"; then
    printf 'error: automation-driver.txt must record primary client-agent bridge wait ready\n' >&2
    exit 1
  fi
  bad_bridge_matches="$(
    awk -F= '/^client_agent_bridge_wait_status_/ && $2 != "ready" { print NR ":" $0 }' "$automation_driver" || true
  )"
  if [[ -n "$bad_bridge_matches" ]]; then
    printf 'error: automation-driver.txt contains non-ready client-agent bridge wait status\n' >&2
    sed -n '1,20p' <<< "$bad_bridge_matches" >&2
    exit 1
  fi
  if grep -Fxq 'second_client_enabled=1' "$automation_driver"; then
    if ! grep -Fxq 'second_client_kind=gradle-runclient' "$automation_driver"; then
      printf 'error: automation-driver.txt must record second_client_kind=gradle-runclient when second_client_enabled=1\n' >&2
      exit 1
    fi
    if ! grep -Fxq 'second_client_adapter_source=auto-gradle-runclient' "$automation_driver"; then
      printf 'error: automation-driver.txt must record second_client_adapter_source=auto-gradle-runclient when second_client_enabled=1\n' >&2
      exit 1
    fi
    if ! grep -Fxq 'second_client_adapter_task=:fabric-agent:runClientAgent' "$automation_driver"; then
      printf 'error: automation-driver.txt must record second_client_adapter_task=:fabric-agent:runClientAgent when second_client_enabled=1\n' >&2
      exit 1
    fi
    if ! grep -Fxq 'second_client_agent_secret=SET_REDACTED' "$automation_driver"; then
      printf 'error: automation-driver.txt must record second_client_agent_secret=SET_REDACTED when second_client_enabled=1\n' >&2
      exit 1
    fi
    if ! grep -Fxq 'client_agent_bridge_wait_status_secondary=ready' "$automation_driver"; then
      printf 'error: automation-driver.txt must record secondary client-agent bridge wait ready when second_client_enabled=1\n' >&2
      exit 1
    fi
    validate_runtime_provenance secondary
  fi
}

validate_automation_driver_for_observations() {
  local automation_driver observations observations_file manifest_file restart_observation two_client_observation
  automation_driver="$1"
  observations="$2"
  observations_file="$3"
  manifest_file="$4"
  restart_observation=0
  two_client_observation=0

  if [[ "$observations" == *'"id":"m94-06-save-restart-after"'* \
    || "$observations" == *'"id":"playable-03-save-restart-after"'* \
    || "$observations" == *'"id":"playable-06-stone-tool-save-restart-after"'* \
    || "$observations" == *'"id":"playable-13-chest-storage-save-restart-after"'* \
    || "$observations" == *'"id":"playable-25-iron-sword-save-restart-after"'* \
    || "$observations" == *'"id":"playable-29-iron-chestplate-save-restart-mitigation-after"'* \
    || "$observations" == *'"id":"playable-45-two-client-shared-chest-save-restart-after"'* \
    || "$observations" == *'"id":"playable-46-generated-ruin-cache-after"'* ]]; then
    restart_observation=1
  fi

  if [[ "$observations" == *'"id":"playable-30-two-client-'* \
    || "$observations" == *'"id":"playable-31-two-client-'* \
    || "$observations" == *'"id":"playable-32-two-client-'* \
    || "$observations" == *'"id":"playable-33-two-client-'* \
    || "$observations" == *'"id":"playable-34-two-client-'* \
    || "$observations" == *'"id":"playable-35-two-client-'* \
    || "$observations" == *'"id":"playable-36-two-client-'* \
    || "$observations" == *'"id":"playable-37-two-client-'* \
    || "$observations" == *'"id":"playable-38-two-client-'* \
    || "$observations" == *'"id":"playable-39-two-client-'* \
    || "$observations" == *'"id":"playable-40-two-client-'* \
    || "$observations" == *'"id":"playable-41-two-client-'* \
    || "$observations" == *'"id":"playable-42-two-client-'* \
    || "$observations" == *'"id":"playable-45-two-client-'* ]]; then
    two_client_observation=1
  fi

  if [[ "$two_client_observation" -eq 1 ]] && ! grep -Fxq 'second_client_enabled=1' "$automation_driver"; then
    printf 'error: automation-driver.txt must record second_client_enabled=1 for two-client observations\n' >&2
    exit 1
  fi
  if [[ "$restart_observation" -eq 1 ]]; then
    if ! grep -Eq '^server_restart_count=[1-9][0-9]*$' "$automation_driver"; then
      printf 'error: automation-driver.txt must record server_restart_count>=1 for after-restart observations\n' >&2
      exit 1
    fi
    if ! grep -Eq '^server_stop_phase=.* signal=INT$' "$automation_driver"; then
      printf 'error: automation-driver.txt must record a graceful server_stop_phase for after-restart observations\n' >&2
      exit 1
    fi
    if ! grep -Eq '^server_exit_phase=.* status=0$' "$automation_driver"; then
      printf 'error: automation-driver.txt must record server_exit_phase with status=0 for after-restart observations\n' >&2
      exit 1
    fi
    if ! grep -Eq '^server_start_phase=.*after' "$automation_driver"; then
      printf 'error: automation-driver.txt must record an after-restart server_start_phase for after-restart observations\n' >&2
      exit 1
    fi
  fi
  if [[ "$observations" == *'"id":"playable-46-generated-ruin-cache-'* ]] \
    && ! grep -Eq '^server_world_dir=.+/world$' "$automation_driver"; then
    printf 'error: playable-46 observations require a runner-created isolated server_world_dir\n' >&2
    exit 1
  fi
  if [[ "$observations" == *'"id":"playable-46-generated-ruin-cache-'* ]] \
    && ! grep -Fxq 'server_op_users=NONE' "$automation_driver"; then
    printf 'error: playable-46 observations require server_op_users=NONE\n' >&2
    exit 1
  fi
  python3 - "$automation_driver" "$observations_file" "$manifest_file" <<'PY'
import json
import sys
from pathlib import Path

automation_driver = Path(sys.argv[1])
observations_path = Path(sys.argv[2])
manifest_path = Path(sys.argv[3])
phase_status_prefix = "client_agent_phase_exit_status_"
passed_phases = set()
for line in automation_driver.read_text().splitlines():
    if not line.startswith(phase_status_prefix) or not line.endswith("=0"):
        continue
    passed_phases.add(line[len(phase_status_prefix) : -2])

manifest = json.loads(manifest_path.read_text())
manifest_scenarios = {
    scenario.get("id")
    for scenario in manifest.get("scenarios", [])
    if isinstance(scenario, dict) and isinstance(scenario.get("id"), str)
}
observations = json.loads(observations_path.read_text())
passed_observed_scenarios = set()
for scenario in observations.get("scenarios", []):
    if not isinstance(scenario, dict) or scenario.get("result") != "passed":
        continue
    scenario_id = scenario.get("id")
    if not isinstance(scenario_id, str):
        continue
    if scenario_id not in manifest_scenarios:
        continue
    passed_observed_scenarios.add(scenario_id)
    if scenario_id not in passed_phases:
        print(
            f"error: automation-driver.txt must record client_agent_phase_exit_status_{scenario_id}=0 for passed observations",
            file=sys.stderr,
        )
        sys.exit(1)
restart_prerequisites = {
    "m94-06-save-restart-after": {"m94-06-save-restart-before"},
    "playable-03-save-restart-after": {
        "playable-03-save-restart-before",
        "playable-03-save-restart-rejoin",
        "playable-04-twenty-minute-survival-loop",
    },
    "playable-06-stone-tool-save-restart-after": {
        "playable-06-stone-tool-save-restart-before",
    },
    "playable-13-chest-storage-save-restart-after": {
        "playable-13-chest-storage-save-restart-before",
    },
    "playable-25-iron-sword-save-restart-after": {
        "playable-25-iron-sword-save-restart-before",
    },
    "playable-29-iron-chestplate-save-restart-mitigation-after": {
        "playable-29-iron-chestplate-save-restart-mitigation-before",
    },
    "playable-45-two-client-shared-chest-save-restart-after": {
        "playable-45-two-client-shared-chest-save-restart-before",
    },
    "playable-46-generated-ruin-cache-after": {
        "playable-46-generated-ruin-cache-before",
    },
}
for after_scenario, required_before_scenarios in restart_prerequisites.items():
    if after_scenario not in passed_observed_scenarios:
        continue
    if passed_observed_scenarios.isdisjoint(required_before_scenarios):
        required = ", ".join(sorted(required_before_scenarios))
        print(
            f"error: observations.json must include a passed before-restart scenario for {after_scenario}: one of {required}",
            file=sys.stderr,
        )
        sys.exit(1)
PY
  python3 "$REPO_ROOT/tools/validate-real-client-restart-evidence.py" \
    "$automation_driver" "$observations_file" "$manifest_file"
}

validate_server_log() {
  local server_log observations matches catastrophic_tick_matches slow_matches
  server_log="$1"
  observations="$2"
  matches="$(
    grep -En \
      'lock wait exceeded.*lock="chunk_prepare"|degraded_delivery=true|teleport confirmation id mismatch|WARN .*dirty chunk cache pressure|dirty pressure flush failed|region changed before replace|chunk preparation abandoned after repeated dirty chunk cache pressure' \
      "$server_log" || true
  )"
  if [[ -n "$matches" ]]; then
    printf 'error: server.log contains playable degradation warning(s)\n' >&2
    sed -n '1,20p' <<< "$matches" >&2
    exit 1
  fi

  catastrophic_tick_matches="$(python3 - "$server_log" <<'PY'
import re
import sys
from pathlib import Path

threshold_us = 500_000
for line_number, line in enumerate(Path(sys.argv[1]).read_text(encoding="utf-8").splitlines(), 1):
    if "runtime tick exceeded performance budget" not in line:
        continue
    match = re.search(r"(?:^|\s)tick_us=(\d+)(?:\s|$)", line)
    if match is None or int(match.group(1)) >= threshold_us:
        print(f"{line_number}:{line}")
PY
)"
  if [[ -n "$catastrophic_tick_matches" ]]; then
    printf 'error: server.log contains catastrophic runtime tick(s) at or above 500000 us\n' >&2
    sed -n '1,20p' <<< "$catastrophic_tick_matches" >&2
    exit 1
  fi

  if [[ "$observations" == *'"id":"playable-41-two-client-chunk-prewarm-crossing"'* || "$observations" == *'"id":"playable-42-two-client-opposite-chunk-crossing"'* ]]; then
    slow_matches="$(
      grep -En \
        'view-distance window flushed.*(slow_fetch_chunks=[1-9][0-9]*|slow_light_compute_chunks=[1-9][0-9]*)' \
        "$server_log" || true
    )"
    if [[ -n "$slow_matches" ]]; then
      printf 'error: server.log contains slow chunk window(s) for prewarm crossing scenario\n' >&2
      sed -n '1,20p' <<< "$slow_matches" >&2
      exit 1
    fi
  fi
}

if [[ "$MODE" == "check" || "$MODE" == "run" ]]; then
  SCENARIO_NO_DEBUG="$(requested_scenario_no_debug)"
fi

if [[ "$MODE" == "run" && -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
  printf 'error: --run requires a graphical display; set DISPLAY or WAYLAND_DISPLAY\n' >&2
  exit 1
fi

if [[ "$MODE" == "check" ]]; then
  client_adapter_status
  if [[ "$SCENARIO_NO_DEBUG" == "1" ]]; then
    printf 'scenario policy: no-debug, no operator privileges\n'
  else
    printf 'scenario policy: debug commands allowed by manifest\n'
  fi
  exit 0
fi

if [[ "$MODE" == "validate-run" ]]; then
  validate_run_dir "$VALIDATE_RUN_DIR"
  exit 0
fi

if [[ "$MODE" == "prepare" ]]; then
  configure_primary_agent_bridge
  configure_second_agent_bridge
  SCENARIO_NO_DEBUG="$(requested_scenario_no_debug 2>/dev/null || printf '1\n')"
  run_dir="$(prepare_run_dir)"
  server_config_source="$(server_config_path)"
  server_config="$run_dir/server.toml"
  fresh_world_dir=""
  if [[ "$FRESH_WORLD" == "1" || "$AGENT_SCENARIO" == "playable-46-generated-ruin-cache" ]]; then
    fresh_world_dir="$run_dir/world"
  fi
  write_real_client_server_config "$server_config_source" "$server_config" "$fresh_world_dir"
  {
    printf 'server_config_source=%s\n' "$server_config_source"
    printf 'server_config_effective=%s\n' "$server_config"
    if [[ -n "$fresh_world_dir" ]]; then
      printf 'server_world_dir=%s\n' "$fresh_world_dir"
    fi
    if [[ -n "$SERVER_SEED" ]]; then
      printf 'server_seed_override=%s\n' "$SERVER_SEED"
    fi
  } >> "$run_dir/automation-driver.txt"
  printf '%s\n' "$run_dir"
  exit 0
fi

if [[ "$MODE" == "run" ]]; then
  client_adapter_status >/dev/null
  if [[ "$CLIENT_KIND" == "gradle-runclient" && -z "$AGENT_SECRET" ]]; then
    AGENT_SECRET="$(python3 - <<'PY'
import secrets
print("s_" + secrets.token_urlsafe(24))
PY
)"
  fi
  if [[ ( "$AGENT_SCENARIO" == "playable-30-two-client-shared-log-drop-pickup" || "$AGENT_SCENARIO" == "playable-31-two-client-earned-shared-chest" || "$AGENT_SCENARIO" == "playable-32-two-client-earned-torch-block-edit" || "$AGENT_SCENARIO" == "playable-33-two-client-player-visibility-movement" || "$AGENT_SCENARIO" == "playable-34-two-client-chat-message" || "$AGENT_SCENARIO" == "playable-35-two-client-player-disconnect-removal" || "$AGENT_SCENARIO" == "playable-36-two-client-player-reconnect-cleanup" || "$AGENT_SCENARIO" == "playable-37-two-client-player-death-respawn-visibility" || "$AGENT_SCENARIO" == "playable-38-two-client-inventory-drop-handoff" || "$AGENT_SCENARIO" == "playable-39-two-client-short-soak" || "$AGENT_SCENARIO" == "playable-40-two-client-chunk-stream-crossing" || "$AGENT_SCENARIO" == "playable-41-two-client-chunk-prewarm-crossing" || "$AGENT_SCENARIO" == "playable-42-two-client-opposite-chunk-crossing" || "$AGENT_SCENARIO" == "playable-45-two-client-shared-chest-save-restart" ) && -z "$SECOND_AGENT_SECRET" ]]; then
    SECOND_AGENT_SECRET="$(python3 - <<'PY'
import secrets
print("s_" + secrets.token_urlsafe(24))
PY
)"
  fi
  configure_primary_agent_bridge
  configure_second_agent_bridge
  run_dir="$(prepare_run_dir)"
  server_config_source="$(server_config_path)"
  server_config="$run_dir/server.toml"
  fresh_world_dir=""
  if [[ "$FRESH_WORLD" == "1" || "$AGENT_SCENARIO" == "playable-46-generated-ruin-cache" ]]; then
    fresh_world_dir="$run_dir/world"
  fi
  if [[ "$AGENT_SCENARIO" == "playable-46-generated-ruin-cache" ]] \
    && ! mkdir "$fresh_world_dir"; then
    printf 'error: playable-46 refuses to reuse an existing world directory: %s\n' "$fresh_world_dir" >&2
    exit 1
  fi
  write_real_client_server_config "$server_config_source" "$server_config" "$fresh_world_dir"
  {
    printf 'server_config_source=%s\n' "$server_config_source"
    printf 'server_config_effective=%s\n' "$server_config"
    printf 'server_command_effective=%s\n' "cargo run --bin mc-server -- --config $server_config"
    if [[ "$SCENARIO_NO_DEBUG" == "1" ]]; then
      printf 'server_op_users=NONE\n'
    else
      printf 'server_op_users=%s,%s\n' "$(client_username_for_label primary)" "$(client_username_for_label secondary)"
    fi
    if [[ -n "$fresh_world_dir" ]]; then
      printf 'server_world_dir=%s\n' "$fresh_world_dir"
    else
      printf 'server_world_dir=UNCHANGED_FROM_SOURCE\n'
    fi
    if [[ -n "$SERVER_SEED" ]]; then
      printf 'server_seed_override=%s\n' "$SERVER_SEED"
    fi
  } >> "$run_dir/automation-driver.txt"
  printf 'running real-client regression into %s\n' "$run_dir"

  server_pid=""
  client_pid=""
  second_client_pid=""
  driver_timeout_seconds=$((CLIENT_TIMEOUT_SECONDS + 15))
  server_restart_count=0
  agent_phase_status=0
  second_client_enabled=0

  start_server() {
    local phase ready_fifo ready_fd ready_result
    phase="$1"
    printf 'server_start_phase=%s\n' "$phase" >> "$run_dir/automation-driver.txt"
    ready_fifo="$run_dir/.server-ready-$phase.fifo"
    rm -f "$ready_fifo"
    mkfifo "$ready_fifo"
    exec {ready_fd}<>"$ready_fifo"
    (
      cd "$REPO_ROOT"
      exec cargo run --bin mc-server -- --config "$server_config"
    ) > >(
      ready_sent=0
      while IFS= read -r line; do
        printf '%s\n' "$line" >> "$run_dir/server.log"
        if [[ "$ready_sent" -eq 0 && "$line" == *"Solaris is listening"* ]]; then
          printf 'ready\n' > "$ready_fifo"
          ready_sent=1
        fi
      done
      if [[ "$ready_sent" -eq 0 ]]; then
        printf 'exited-before-ready\n' > "$ready_fifo"
      fi
    ) 2>&1 &
    server_pid="$!"
    printf 'server_pid_%s=%s\n' "$phase" "$server_pid" >> "$run_dir/automation-driver.txt"
    ready_result=""
    if ! IFS= read -r -t "$SERVER_READY_TIMEOUT_SECONDS" ready_result <&"$ready_fd"; then
      ready_result="timeout"
    fi
    exec {ready_fd}>&-
    rm -f "$ready_fifo"
    printf 'server_ready_phase=%s status=%s\n' "$phase" "$ready_result" >> "$run_dir/automation-driver.txt"
    if [[ "$ready_result" != "ready" ]]; then
      printf 'error: server did not publish readiness for phase %s: %s\n' "$phase" "$ready_result" >&2
      return 1
    fi
  }

  stop_server_gracefully() {
    local phase require_clean_stop status
    phase="$1"
    require_clean_stop="${2:-0}"
    if [[ -z "${server_pid:-}" ]]; then
      [[ "$require_clean_stop" -eq 0 ]]
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
    if [[ "$require_clean_stop" -eq 1 ]]; then
      return "$status"
    fi
    return 0
  }

  run_agent_driver_phase() {
    local scenario phase_status
    local -a replay_args
    scenario="$1"
    shift
    replay_args=()
    if [[ -f "$run_dir/core-replay-manifest.json" ]]; then
      replay_args+=(--replay-manifest "$run_dir/core-replay-manifest.json")
    fi
    printf 'client_agent_phase_scenario=%s\n' "$scenario" >> "$run_dir/automation-driver.txt"
    set +e
    timeout "$driver_timeout_seconds" python3 "$AGENT_DRIVER" \
      --bridge-url "$AGENT_BRIDGE_URL" \
      --secret="$AGENT_SECRET" \
      --run-dir "$run_dir" \
      --scenario "$scenario" \
      --server-addr "$SERVER_ADDR" \
      --timeout-seconds "$CLIENT_TIMEOUT_SECONDS" \
      "${replay_args[@]}" \
      "$@" >> "$run_dir/automation-driver.txt" 2>&1
    phase_status="$?"
    set -e
    agent_phase_status="$phase_status"
    printf 'client_agent_phase_exit_status_%s=%s\n' "$scenario" "$phase_status" >> "$run_dir/automation-driver.txt"
    return 0
  }

  validate_client_agent_runtime_provenance() {
    local label provenance_file expected_classes_dir provenance_result provenance_status
    local artifact_kind artifact_path artifact_sha256 artifact_file_count
    label="$1"
    provenance_file="$run_dir/client-agent-runtime-$label.txt"
    expected_classes_dir="$AGENT_ROOT/fabric-agent/build/classes/java/main"
    set +e
    provenance_result="$(python3 - "$provenance_file" "$expected_classes_dir" <<'PY'
import hashlib
import sys
from pathlib import Path

provenance_file = Path(sys.argv[1])
expected_classes_dir = Path(sys.argv[2]).resolve()
required = {
    "runtime_artifact_kind",
    "runtime_artifact_path",
    "runtime_artifact_sha256",
    "runtime_artifact_file_count",
}

try:
    entries = {}
    for line in provenance_file.read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition("=")
        if not separator or key in entries:
            raise ValueError(f"invalid runtime provenance line: {line!r}")
        entries[key] = value
    if set(entries) != required:
        raise ValueError("runtime provenance fields do not match the required schema")
    if entries["runtime_artifact_kind"] != "compiled-classes":
        raise ValueError("runtime artifact kind must be compiled-classes")
    artifact_path = Path(entries["runtime_artifact_path"]).resolve()
    if artifact_path != expected_classes_dir:
        raise ValueError(f"runtime artifact path must be {expected_classes_dir}, got {artifact_path}")
    if not artifact_path.is_dir():
        raise ValueError(f"runtime artifact directory is missing: {artifact_path}")
    files = sorted(
        (path for path in artifact_path.rglob("*") if path.is_file()),
        key=lambda path: path.relative_to(artifact_path).as_posix(),
    )
    if not files:
        raise ValueError("runtime artifact directory is empty")
    if entries["runtime_artifact_file_count"] != str(len(files)):
        raise ValueError("runtime artifact file count does not match the compiled classes directory")
    digest = hashlib.sha256()
    for path in files:
        digest.update(path.relative_to(artifact_path).as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
    if entries["runtime_artifact_sha256"] != digest.hexdigest():
        raise ValueError("runtime artifact digest does not match the compiled classes directory")
except (OSError, ValueError) as exc:
    print(f"error: {exc}")
    sys.exit(1)

for key in sorted(required):
    print(f"{key}={entries[key]}")
PY
)"
    provenance_status="$?"
    set -e
    if [[ "$provenance_status" -ne 0 ]]; then
      printf 'client_agent_runtime_validation_%s=failed\n' "$label" >> "$run_dir/automation-driver.txt"
      printf 'client_agent_runtime_validation_error_%s=%s\n' "$label" "$provenance_result" >> "$run_dir/automation-driver.txt"
      printf 'error: %s client runtime classes provenance validation failed: %s\n' "$label" "$provenance_result" >&2
      return 1
    fi
    artifact_kind="$(sed -n 's/^runtime_artifact_kind=//p' <<< "$provenance_result")"
    artifact_path="$(sed -n 's/^runtime_artifact_path=//p' <<< "$provenance_result")"
    artifact_sha256="$(sed -n 's/^runtime_artifact_sha256=//p' <<< "$provenance_result")"
    artifact_file_count="$(sed -n 's/^runtime_artifact_file_count=//p' <<< "$provenance_result")"
    {
      printf 'client_agent_runtime_kind_%s=%s\n' "$label" "$artifact_kind"
      printf 'client_agent_runtime_path_%s=%s\n' "$label" "$artifact_path"
      printf 'client_agent_runtime_sha256_%s=%s\n' "$label" "$artifact_sha256"
      printf 'client_agent_runtime_file_count_%s=%s\n' "$label" "$artifact_file_count"
      printf 'client_agent_runtime_validation_%s=verified\n' "$label"
    } >> "$run_dir/automation-driver.txt"
  }

  wait_for_agent_bridge() {
    local label bridge_url secret timeout wait_result wait_status wait_seconds
    label="$1"
    bridge_url="$2"
    secret="$3"
    timeout="$4"
    set +e
    wait_result="$(python3 - "$bridge_url" "$secret" "$timeout" <<'PY'
import json
import sys
import time
from urllib import request

bridge_url = sys.argv[1]
secret = sys.argv[2]
timeout_seconds = float(sys.argv[3])
started = time.monotonic()
payload = json.dumps({
    "id": 1,
    "secret": secret,
    "command": "ping",
    "payload": {},
}, separators=(",", ":")).encode("utf-8")
rpc_request = request.Request(
    bridge_url,
    data=payload,
    headers={"Content-Type": "application/json"},
    method="POST",
)
try:
    with request.urlopen(rpc_request, timeout=timeout_seconds) as response:
        decoded = json.loads(response.read().decode("utf-8"))
except Exception as exc:
    print(f"unavailable {time.monotonic() - started:.3f} {exc}")
    sys.exit(1)
if decoded.get("ok") is not True:
    print(f"invalid {time.monotonic() - started:.3f} {json.dumps(decoded, sort_keys=True)}")
    sys.exit(1)
print(f"ready {time.monotonic() - started:.3f}")
PY
)"
    wait_status="$?"
    set -e
    wait_seconds="$(printf '%s\n' "$wait_result" | awk '{print $2}')"
    if [[ "$wait_status" -eq 0 ]]; then
      if ! validate_client_agent_runtime_provenance "$label"; then
        return 1
      fi
      printf 'client_agent_bridge_wait_status_%s=ready\n' "$label" >> "$run_dir/automation-driver.txt"
      printf 'client_agent_bridge_wait_seconds_%s=%s\n' "$label" "$wait_seconds" >> "$run_dir/automation-driver.txt"
      return 0
    fi
    printf 'client_agent_bridge_wait_status_%s=timeout\n' "$label" >> "$run_dir/automation-driver.txt"
    printf 'client_agent_bridge_wait_seconds_%s=%s\n' "$label" "${wait_seconds:-$timeout}" >> "$run_dir/automation-driver.txt"
    printf 'client_agent_bridge_wait_error_%s=%s\n' "$label" "$wait_result" >> "$run_dir/automation-driver.txt"
    return 1
  }

  launch_gradle_runclient() {
    local label port secret log_file pid_var jdk_options client_game_dir client_username runtime_provenance_file
    local ready_fifo ready_fd ready_result
    label="$1"
    port="$2"
    secret="$3"
    log_file="$4"
    pid_var="$5"
    jdk_options="${JDK_JAVA_OPTIONS:-}"
    client_game_dir="$(client_game_dir_for_label "$label")"
    client_username="$(client_username_for_label "$label")"
    mkdir -p "$client_game_dir"
    : > "$log_file"
    ready_fifo="$run_dir/.client-agent-ready-$label.fifo"
    rm -f "$ready_fifo"
    mkfifo "$ready_fifo"
    exec {ready_fd}<>"$ready_fifo"
    runtime_provenance_file="$run_dir/client-agent-runtime-$label.txt"
    rm -f "$runtime_provenance_file"
    printf 'client_agent_runtime_entrypoint_%s=GRADLE_RUNCLIENT_ENTRYPOINT\n' "$label" >> "$run_dir/automation-driver.txt"
    printf 'client_game_dir_%s=%s\n' "$label" "$client_game_dir" >> "$run_dir/automation-driver.txt"
    printf 'client_username_%s=%s\n' "$label" "$client_username" >> "$run_dir/automation-driver.txt"
    (
      cd "$AGENT_ROOT"
      SOLARIS_CLIENT_AGENT_SECRET="$secret" \
        SOLARIS_CLIENT_AGENT_PORT="$port" \
        SOLARIS_CLIENT_AGENT_RUN_DIR="$run_dir" \
        SOLARIS_CLIENT_AGENT_GAME_DIR="$client_game_dir" \
        SOLARIS_CLIENT_AGENT_USERNAME="$client_username" \
        JDK_JAVA_OPTIONS="$jdk_options" \
        ./gradlew --no-configuration-cache "$GRADLE_RUNCLIENT_TASK" \
          -Psolaris.clientAgent.secret="$secret" \
          -Psolaris.clientAgent.port="$port" \
          -Psolaris.clientAgent.runDir="$run_dir" \
          -Psolaris.clientAgent.gameDir="$client_game_dir" \
          -Psolaris.clientAgent.runtimeProvenanceFile="$runtime_provenance_file" \
          -Psolaris.clientAgent.username="$client_username"
    ) > >(
      ready_sent=0
      while IFS= read -r line; do
        printf '%s\n' "$line" >> "$log_file"
        if [[ "$ready_sent" -eq 0 \
          && "$line" == *"Solaris client agent bridge listening on http://127.0.0.1:$port/rpc"* ]]; then
          printf 'ready\n' > "$ready_fifo"
          ready_sent=1
        fi
      done
      if [[ "$ready_sent" -eq 0 ]]; then
        printf 'exited-before-ready\n' > "$ready_fifo"
      fi
    ) 2>&1 &
    printf -v "$pid_var" '%s' "$!"
    printf 'client_pid_%s=%s\n' "$label" "${!pid_var}" >> "$run_dir/automation-driver.txt"
    ready_result=""
    if ! IFS= read -r -t "$CLIENT_TIMEOUT_SECONDS" ready_result <&"$ready_fd"; then
      ready_result="timeout"
    fi
    exec {ready_fd}>&-
    rm -f "$ready_fifo"
    printf 'client_agent_process_ready_%s=%s\n' "$label" "$ready_result" >> "$run_dir/automation-driver.txt"
    if [[ "$ready_result" != "ready" ]]; then
      printf 'error: %s client bridge process did not publish readiness: %s\n' "$label" "$ready_result" >&2
      return 1
    fi
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

    launch_gradle_runclient "primary" "$AGENT_PORT" "$AGENT_SECRET" "$run_dir/client.log" client_pid
    if ! wait_for_agent_bridge "primary" "$AGENT_BRIDGE_URL" "$AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
      driver_status=1
    else

    if [[ "$AGENT_SCENARIO" == "m94-06-save-restart-two-client-visibility" ]]; then
      run_agent_driver_phase "m94-06-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "m94-06-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "m94-06-after-restart"
        run_agent_driver_phase "m94-06-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
        if [[ "$second_client_enabled" -eq 1 ]]; then
          launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
          if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
            run_agent_driver_phase "m94-06-two-client-live-visibility" \
              --append-observations \
              --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
              --secondary-secret="$SECOND_AGENT_SECRET"
            printf 'client_agent_two_client_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
            run_agent_driver_phase "m94-06-two-client-shared-drop" \
              --append-observations \
              --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
              --secondary-secret="$SECOND_AGENT_SECRET"
            printf 'client_agent_two_client_shared_drop_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
            run_agent_driver_phase "m94-06-two-client-shared-pickup" \
              --append-observations \
              --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
              --secondary-secret="$SECOND_AGENT_SECRET"
            printf 'client_agent_two_client_shared_pickup_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
          fi
          driver_status=1
        fi
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-03-save-restart-rejoin" ]]; then
      run_agent_driver_phase "playable-03-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-03-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-03-after-restart"
        run_agent_driver_phase "playable-03-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-04-twenty-minute-survival-loop" ]]; then
      run_agent_driver_phase "playable-04-twenty-minute-survival-loop"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-04-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-04-after-restart"
        run_agent_driver_phase "playable-03-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-06-stone-tool-save-restart" ]]; then
      run_agent_driver_phase "playable-06-stone-tool-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-06-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-06-after-restart"
        run_agent_driver_phase "playable-06-stone-tool-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-13-chest-storage-save-restart" ]]; then
      run_agent_driver_phase "playable-13-chest-storage-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-13-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-13-after-restart"
        run_agent_driver_phase "playable-13-chest-storage-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-25-iron-sword-save-restart" ]]; then
      run_agent_driver_phase "playable-25-iron-sword-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-25-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-25-after-restart"
        run_agent_driver_phase "playable-25-iron-sword-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-29-iron-chestplate-save-restart-mitigation" ]]; then
      run_agent_driver_phase "playable-29-iron-chestplate-save-restart-mitigation-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-29-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-29-after-restart"
        run_agent_driver_phase "playable-29-iron-chestplate-save-restart-mitigation-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-45-two-client-shared-chest-save-restart" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "playable-45-two-client-shared-chest-save-restart-before" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
          driver_status="$agent_phase_status"
          if [[ "$driver_status" -eq 0 ]]; then
            stop_server_gracefully "playable-45-before-restart" 1
            server_restart_count=1
            printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
            start_server "playable-45-after-restart"
            run_agent_driver_phase "playable-45-two-client-shared-chest-save-restart-after" \
              --append-observations \
              --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
              --secondary-secret="$SECOND_AGENT_SECRET"
            driver_status="$agent_phase_status"
          fi
        else
          driver_status=1
        fi
        printf 'client_agent_playable_two_client_shared_chest_restart_phase_exit_status=%s\n' "$driver_status" >> "$run_dir/automation-driver.txt"
      else
        printf 'error: playable-45 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-46-generated-ruin-cache" ]]; then
      run_agent_driver_phase "playable-46-generated-ruin-cache-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        if stop_server_gracefully "playable-46-before-restart" 1; then
          server_restart_count=1
          printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
          start_server "playable-46-after-restart"
          run_agent_driver_phase "playable-46-generated-ruin-cache-after" --append-observations
          driver_status="$agent_phase_status"
        else
          printf 'error: playable-46 clean server stop failed; refusing after-restart phase\n' >&2
          driver_status=1
        fi
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-30-two-client-shared-log-drop-pickup" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_shared_drop_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-30 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-31-two-client-earned-shared-chest" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_shared_chest_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-31 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-32-two-client-earned-torch-block-edit" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_block_edit_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-32 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-33-two-client-player-visibility-movement" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_player_visibility_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-33 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-34-two-client-chat-message" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_chat_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-34 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-35-two-client-player-disconnect-removal" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_player_disconnect_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-35 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-36-two-client-player-reconnect-cleanup" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_player_reconnect_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-36 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-37-two-client-player-death-respawn-visibility" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_player_death_respawn_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-37 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-38-two-client-inventory-drop-handoff" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_inventory_drop_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-38 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-39-two-client-short-soak" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_short_soak_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-39 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-40-two-client-chunk-stream-crossing" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_chunk_crossing_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-40 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-41-two-client-chunk-prewarm-crossing" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_chunk_prewarm_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-41 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-42-two-client-opposite-chunk-crossing" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_opposite_chunk_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-42 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "m94-03b-two-client-shared-chest" || "$AGENT_SCENARIO" == "m94-03c-two-client-shared-chest-live-update" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
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
    validate_run_dir "$run_dir"
    printf 'validate with: bash tools/run-real-client-regression.sh --validate-run %s\n' "$run_dir"
    exit "$driver_status"
  fi

  printf 'error: primary Gradle runClient adapter did not configure the in-client bridge\n' >&2
  exit 1
fi
