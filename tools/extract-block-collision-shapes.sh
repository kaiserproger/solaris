#!/usr/bin/env bash
# Extract state-specific collision boxes for every block state from the bundled
# vanilla 26.1.2 server. The generated table is embedded in mc-data.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECK=0
if [[ "${1:-}" == "--check" ]]; then
  CHECK=1
  shift
fi

BUNDLE_JAR="${1:-$REPO_ROOT/.analysis/server.jar}"
OUT_BIN="$REPO_ROOT/crates/mc-data/data/block_collision_shapes_26_1_2.bin"
JAVA="${JAVA:-$REPO_ROOT/.analysis/java/current/bin/java}"
if [[ ! -x "$JAVA" ]]; then
  JAVA="$(command -v java || true)"
fi
JAVAC="${JAVA%/java}/javac"

if [[ ! -f "$BUNDLE_JAR" ]]; then
  echo "error: bundle jar not found at $BUNDLE_JAR" >&2
  exit 1
fi
if [[ ! -x "$JAVA" ]] || [[ ! -x "$JAVAC" ]]; then
  echo "error: java/javac not found (java=$JAVA)" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

unzip -qo "$BUNDLE_JAR" -d "$TMP/bundle"
INNER_JAR="$(find "$TMP/bundle/META-INF/versions" -name 'server-*.jar' | head -n1)"
if [[ -z "$INNER_JAR" ]]; then
  echo "error: could not locate inner server-<version>.jar" >&2
  exit 1
fi

LIB_CP="$(find "$TMP/bundle/META-INF/libraries" -name '*.jar' | paste -sd:)"
CP="$INNER_JAR:$LIB_CP"
mkdir -p "$TMP/classes"
"$JAVAC" -d "$TMP/classes" -cp "$CP" \
  "$REPO_ROOT/tools/extract-block-collision-shapes/CollisionShapeExtractor.java"

GENERATED="$TMP/block_collision_shapes_26_1_2.bin"
"$JAVA" -cp "$TMP/classes:$CP" CollisionShapeExtractor "$GENERATED"

if [[ "$CHECK" -eq 1 ]]; then
  if ! cmp -s "$GENERATED" "$OUT_BIN"; then
    echo "error: $OUT_BIN is stale; rerun tools/extract-block-collision-shapes.sh" >&2
    exit 1
  fi
  echo "verified $OUT_BIN against bundled vanilla 26.1.2"
else
  mkdir -p "$(dirname "$OUT_BIN")"
  cp "$GENERATED" "$OUT_BIN"
  echo "wrote $OUT_BIN ($(stat -c %s "$OUT_BIN") bytes)"
fi
