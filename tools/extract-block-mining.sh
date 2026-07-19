#!/usr/bin/env bash
# Extract per-state mining hardness and correct-tool requirements from the
# bundled vanilla server. Output is local sidecar data, not source.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_JAR="${1:-$REPO_ROOT/.analysis/server.jar}"
OUT_JSON="$REPO_ROOT/data/vanilla/reports/block_mining.json"
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

echo "[1/4] Unpacking bundle..."
unzip -qo "$BUNDLE_JAR" -d "$TMP/bundle"
INNER_JAR="$(find "$TMP/bundle/META-INF/versions" -name 'server-*.jar' | head -n1)"
if [[ -z "$INNER_JAR" ]]; then
  echo "error: could not locate inner server-<version>.jar" >&2
  exit 1
fi

LIB_CP="$(find "$TMP/bundle/META-INF/libraries" -name '*.jar' | paste -sd:)"
CP="$INNER_JAR:$LIB_CP"

echo "[2/4] Compiling MiningExtractor..."
mkdir -p "$TMP/classes"
"$JAVAC" -d "$TMP/classes" -cp "$CP" \
  "$REPO_ROOT/tools/extract-block-mining/MiningExtractor.java"

echo "[3/4] Running extractor..."
mkdir -p "$(dirname "$OUT_JSON")"
"$JAVA" -cp "$TMP/classes:$CP" MiningExtractor "$OUT_JSON"

echo "[4/4] Wrote $OUT_JSON ($(stat -c %s "$OUT_JSON") bytes)"
