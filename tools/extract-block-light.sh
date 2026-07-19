#!/usr/bin/env bash
# Per ADR 0001: dump per-block-state light metadata (emission,
# dampening/opacity, sky propagation, suffocation) out of the vanilla 26.1.x server
# jar so Solaris's lighting engine has its block table. Mojang's
# `--reports` mode doesn't expose these fields, so we run a custom
# main class against the staged classpath. Output is data, not source.
#
# Usage:
#   tools/extract-block-light.sh [path/to/server.jar]
#
# Default source: .analysis/server.jar (the bundle JAR — the script
# walks past the bundler layer to the embedded version-specific jar).
#
# Output: data/vanilla/reports/block_light.json (gitignored).
#
# Requires Java matching the server's java_version (25 for 26.1.x).
# Override with JAVA=...; defaults to the sdkman 25.0.x install.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_JAR="${1:-$REPO_ROOT/.analysis/server.jar}"
OUT_JSON="$REPO_ROOT/data/vanilla/reports/block_light.json"
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
  echo "       set JAVA=/path/to/jdk25/bin/java" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "[1/4] Unpacking bundle…"
unzip -qo "$BUNDLE_JAR" -d "$TMP/bundle"
INNER_JAR="$(find "$TMP/bundle/META-INF/versions" -name 'server-*.jar' | head -n1)"
if [[ -z "$INNER_JAR" ]]; then
  echo "error: could not locate inner server-<version>.jar" >&2
  exit 1
fi

LIB_CP="$(find "$TMP/bundle/META-INF/libraries" -name '*.jar' | paste -sd:)"
CP="$INNER_JAR:$LIB_CP"

echo "[2/4] Compiling LightExtractor…"
mkdir -p "$TMP/classes"
"$JAVAC" -d "$TMP/classes" -cp "$CP" \
  "$REPO_ROOT/tools/extract-block-light/LightExtractor.java"

echo "[3/4] Running extractor (this triggers vanilla's Bootstrap, takes ~10s)…"
mkdir -p "$(dirname "$OUT_JSON")"
"$JAVA" -cp "$TMP/classes:$CP" LightExtractor "$OUT_JSON"

echo "[4/4] Done."
bytes=$(stat -c %s "$OUT_JSON")
echo "    wrote $OUT_JSON ($bytes bytes)"
