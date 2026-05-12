#!/usr/bin/env bash
# Per ADR 0002: extract packet-ID registration tables and packet record
# layouts from the unobfuscated vanilla server/client jar via `javap`.
# Output goes to .analysis/protocol-dump.txt (gitignored).
#
# Usage:
#   tools/dump-vanilla-protocol.sh [path/to/client.jar]
#
# Default source: client.jar from a parallel download cache under
# /tmp/mc-client-26.1.2 (set up by the M1.g.4 work); falls back to the
# bundled .analysis/server.jar's inner server-<version>.jar.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JAR="${1:-/tmp/mc-client-26.1.2/client.jar}"
JAVA25="${JAVA25:-/home/user/.sdkman/candidates/java/25.0.2-graalce/bin/java}"
JAVAP="${JAVA25%/java}/javap"
OUT="$REPO_ROOT/.analysis/protocol-dump.txt"

if [[ ! -f "$JAR" ]]; then
  echo "error: jar not found at $JAR" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$(dirname "$OUT")"

unzip -qo "$JAR" 'net/minecraft/network/protocol/*' -d "$TMP"

PROTOCOLS=(
  "handshake/HandshakeProtocols"
  "status/StatusProtocols"
  "login/LoginProtocols"
  "configuration/ConfigurationProtocols"
  "game/GameProtocols"
)

{
  echo "# Vanilla 26.1.2 protocol ID registration tables"
  echo "# Extracted from $(basename "$JAR") via javap, per ADR 0002."
  echo "# The numerical ID is the zero-based position within each direction."
  echo
  for proto in "${PROTOCOLS[@]}"; do
    cls="$TMP/net/minecraft/network/protocol/$proto.class"
    [[ -f "$cls" ]] || continue
    echo "=================================================================="
    echo "== $proto"
    echo "=================================================================="
    "$JAVAP" -p -c "$cls" \
      | grep -E 'addPacket|Field.*PacketType' \
      | awk '
          /Field.*PacketType/ {
            # Extract just the PACKET_NAME at the end of "Field net/.../FooPacketTypes.PACKET_NAME:..."
            n = split($0, parts, "[./]")
            for (i = 1; i <= n; i++) {
              if (parts[i] ~ /^[A-Z_]+:L/) {
                sub(/:L.*/, "", parts[i])
                print parts[i]
              }
            }
          }
        '
    echo
  done
} > "$OUT"

# Append a per-packet record summary (field names + types in declared order).
{
  echo "=================================================================="
  echo "== Per-packet record fields (declared order = on-wire order)"
  echo "=================================================================="
  echo
  for state in configuration login game handshake status common cookie ping; do
    dir="$TMP/net/minecraft/network/protocol/$state"
    [[ -d "$dir" ]] || continue
    for cls in "$dir"/*Packet.class; do
      [[ -f "$cls" ]] || continue
      name=$(basename "$cls" .class)
      # Skip inner/nested classes.
      [[ "$name" == *"\$"* ]] && continue
      echo "---- $state/$name"
      "$JAVAP" -p "$cls" \
        | awk '
            /private final/ {
              # "private final SomeType name;"
              sub(/^[[:space:]]+/, "")
              sub(/;$/, "")
              print "    " $0
            }
          '
    done
  done
} >> "$OUT"

lines=$(wc -l < "$OUT")
size=$(wc -c < "$OUT")
echo "wrote $OUT ($lines lines, $size bytes)"
