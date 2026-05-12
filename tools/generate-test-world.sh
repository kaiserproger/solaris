#!/usr/bin/env bash
# Generate a small vanilla 26.1.2 flat world by briefly running the
# bundled server. Output lands in .analysis/test-world/ (gitignored)
# for use as the round-trip oracle in M2.e Anvil tests.
#
# Usage:
#   tools/generate-test-world.sh
#
# Idempotent: if .analysis/test-world/region/r.0.0.mca already exists
# the script exits early. Delete .analysis/test-world/ to force a
# regeneration.
#
# Requires Java matching the bundled server's java_version (25 for
# 26.1.x); override with JAVA=…. Same JDK as
# tools/extract-vanilla-data.sh.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_JAR="${1:-$REPO_ROOT/.analysis/server.jar}"
OUT_DIR="$REPO_ROOT/.analysis/test-world"
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
trap 'rm -rf "$RUN_DIR"' EXIT

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

echo "[2/4] Starting server, waiting for save → SIGINT → kill …"
LOG="$RUN_DIR/server.log"
WORLD="$RUN_DIR/world"
# In 26.1 the Overworld lives under dimensions/minecraft/overworld/.
# (Pre-1.20-ish layouts had region/ at world/region/.)
REGION_DIR="$WORLD/dimensions/minecraft/overworld/region"
REGION_FILE="$REGION_DIR/r.0.0.mca"
(
  cd "$RUN_DIR"
  "$JAVA" -Xmx512M -jar "$BUNDLE_JAR" --nogui >"$LOG" 2>&1 &
  echo $! > server.pid
)
PID="$(cat "$RUN_DIR/server.pid")"

# Wait up to 90 s for spawn save: the server runs synchronously
# through worldgen of the spawn chunks before the "Done" line, but
# it doesn't flush region files until shortly after. As soon as
# the "Done" line lands, ask it to save and stop.
done_seen=0
for _ in $(seq 1 90); do
  if grep -q 'Done (' "$LOG" 2>/dev/null; then
    done_seen=1
    echo "[3/4] Server up; sending SIGINT (graceful save) …"
    kill -INT "$PID" || true
    break
  fi
  sleep 1
done
if [[ $done_seen -eq 0 ]]; then
  echo "error: server never printed 'Done ('; tail of log:" >&2
  tail -30 "$LOG" >&2
  kill -KILL "$PID" 2>/dev/null || true
  exit 1
fi

# Poll for the region file to appear and be non-empty, up to 60 s.
# As soon as we see it stop growing for 2 s we're done.
last_size=0
stable_for=0
for _ in $(seq 1 60); do
  if [[ -f "$REGION_FILE" ]]; then
    cur_size="$(stat -c %s "$REGION_FILE")"
    if [[ "$cur_size" -gt 0 && "$cur_size" -eq "$last_size" ]]; then
      stable_for=$((stable_for + 1))
      if [[ $stable_for -ge 2 ]]; then
        break
      fi
    else
      stable_for=0
    fi
    last_size="$cur_size"
  fi
  sleep 1
done

if [[ ! -s "$REGION_FILE" ]]; then
  echo "error: $REGION_FILE not produced; tail of server log:" >&2
  tail -30 "$LOG" >&2
  kill -KILL "$PID" 2>/dev/null || true
  exit 1
fi

# Kill the server immediately — we have what we need.
kill -KILL "$PID" 2>/dev/null || true
# Give the OS a moment to flush file descriptors.
sleep 1

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
