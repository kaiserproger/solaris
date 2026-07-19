#!/usr/bin/env bash
# Generate a small vanilla 26.1.2 flat world by briefly running the
# bundled server. Output lands in .analysis/test-world/ by default
# (gitignored) for use as the round-trip oracle in Anvil tests.
#
# Usage:
#   tools/generate-test-world.sh
#
# Idempotent: if .analysis/test-world/region/r.0.0.mca already exists
# the script exits early. Delete .analysis/test-world/ to force a
# regeneration.
#
# Override OUT_DIR=... to write another local oracle world, and
# REGION_FILE_COMPRESSION=... to set vanilla's region-file-compression.
# Requires Java matching the bundled server's java_version (25 for
# 26.1.x); override with JAVA=…. Same JDK as
# tools/extract-vanilla-data.sh.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_JAR="${1:-$REPO_ROOT/.analysis/server.jar}"
OUT_DIR="${OUT_DIR:-$REPO_ROOT/.analysis/test-world}"
JAVA="${JAVA:-/home/user/.sdkman/candidates/java/25.0.2-graalce/bin/java}"

if [[ -f "$OUT_DIR/region/r.0.0.mca" ]]; then
  echo "[skip] $OUT_DIR/region/r.0.0.mca already exists"
  exit 0
fi
if [[ ! -f "$BUNDLE_JAR" ]]; then
  echo "error: bundle jar not found at $BUNDLE_JAR" >&2
  exit 1
fi
if [[ ! -x "$JAVA" ]]; then
  echo "error: java not found at $JAVA (override with JAVA=…)" >&2
  exit 1
fi

RUN_DIR="$(mktemp -d)"
PID=""
cleanup() {
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill -KILL "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$RUN_DIR"
}
trap cleanup EXIT

echo "[1/4] Preparing run dir at $RUN_DIR …"
echo "eula=true" > "$RUN_DIR/eula.txt"
cat > "$RUN_DIR/server.properties" <<'EOF'
level-type=minecraft\:flat
level-name=world
online-mode=false
white-list=false
max-players=1
view-distance=2
simulation-distance=2
spawn-protection=0
generate-structures=false
spawn-monsters=false
spawn-animals=false
spawn-npcs=false
motd=Solaris test world
gamemode=creative
difficulty=peaceful
server-port=0
sync-chunk-writes=true
EOF
if [[ -n "${REGION_FILE_COMPRESSION:-}" ]]; then
  printf 'region-file-compression=%s\n' "$REGION_FILE_COMPRESSION" >> "$RUN_DIR/server.properties"
fi

echo "[2/4] Starting server and waiting for listener readiness …"
LOG="$RUN_DIR/server.log"
WORLD="$RUN_DIR/world"
# In 26.1 the Overworld lives under dimensions/minecraft/overworld/.
# (Pre-1.20-ish layouts had region/ at world/region/.)
REGION_DIR="$WORLD/dimensions/minecraft/overworld/region"
REGION_FILE="$REGION_DIR/r.0.0.mca"
coproc VANILLA_SERVER {
  cd "$RUN_DIR"
  exec "$JAVA" -Xmx512M -jar "$BUNDLE_JAR" --nogui 2>&1
}
PID="$VANILLA_SERVER_PID"
SERVER_OUTPUT_FD="${VANILLA_SERVER[0]}"

# Read the process pipe directly. Each log line wakes this loop; the timeout
# only fails a server that stops producing readiness evidence.
done_seen=0
while true; do
  if IFS= read -r -t 90 line <&"$SERVER_OUTPUT_FD"; then
    printf '%s\n' "$line" >> "$LOG"
    if [[ "$line" == *'Done ('* ]]; then
      done_seen=1
      echo "[3/4] Server up; sending SIGINT for graceful save …"
      kill -INT "$PID"
      break
    fi
  else
    read_status=$?
    if [[ $read_status -gt 128 ]]; then
      echo "error: timed out waiting for the next server readiness event" >&2
    else
      echo "error: server output closed before readiness" >&2
    fi
    break
  fi
done
if [[ $done_seen -eq 0 ]]; then
  echo "error: server never printed 'Done ('; tail of log:" >&2
  tail -30 "$LOG" >&2
  kill -KILL "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null || true
  PID=""
  exit 1
fi

# Continue draining the exact process output until EOF. A timeout only kills
# a shutdown that is stuck; elapsed time is never treated as successful save.
while true; do
  if IFS= read -r -t 60 line <&"$SERVER_OUTPUT_FD"; then
    printf '%s\n' "$line" >> "$LOG"
    continue
  fi
  read_status=$?
  if [[ $read_status -gt 128 ]]; then
    echo "error: server did not close its output after SIGINT; tail of log:" >&2
    tail -30 "$LOG" >&2
    kill -KILL "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    exit 1
  fi
  break
done

set +e
wait "$PID"
server_status=$?
set -e
PID=""
if [[ $server_status -ne 0 && $server_status -ne 130 ]]; then
  echo "error: server exited unexpectedly with status $server_status; tail of log:" >&2
  tail -30 "$LOG" >&2
  exit 1
fi

if [[ ! -s "$REGION_FILE" ]]; then
  echo "error: $REGION_FILE not produced by graceful save; tail of server log:" >&2
  tail -30 "$LOG" >&2
  exit 1
fi

echo "[4/4] Copying world to $OUT_DIR …"
mkdir -p "$OUT_DIR"
rm -rf "$OUT_DIR/region" "$OUT_DIR/entities" "$OUT_DIR/poi" "$OUT_DIR/level.dat"
DIM="$WORLD/dimensions/minecraft/overworld"
cp -r "$DIM/region" "$OUT_DIR/region"
[[ -d "$DIM/entities" ]] && cp -r "$DIM/entities" "$OUT_DIR/entities"
[[ -d "$DIM/poi" ]]       && cp -r "$DIM/poi" "$OUT_DIR/poi"
cp "$WORLD/level.dat" "$OUT_DIR/level.dat"

region_count=$(find "$OUT_DIR/region" -name 'r.*.mca' -size +0c | wc -l)
total_bytes=$(du -sb "$OUT_DIR" | awk '{print $1}')
echo "    wrote $region_count region file(s), $total_bytes bytes ($(du -sh "$OUT_DIR" | awk '{print $1}'))"
