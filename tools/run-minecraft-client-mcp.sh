#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
AGENT_ROOT="$REPO_ROOT/client-mod/solaris-client-agent"
MODE="run"

usage() {
    printf '%s\n' 'Usage: tools/run-minecraft-client-mcp.sh [--check]'
}

if [[ $# -gt 1 ]]; then
    usage >&2
    exit 2
fi
if [[ $# -eq 1 ]]; then
    case "$1" in
        --check)
            MODE="check"
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf 'Unknown argument: %s\n' "$1" >&2
            usage >&2
            exit 2
            ;;
    esac
fi

: "${SOLARIS_CLIENT_MCP_TOKEN:?Set SOLARIS_CLIENT_MCP_TOKEN to a random bearer token.}"

SOLARIS_CLIENT_MCP_PORT="${SOLARIS_CLIENT_MCP_PORT:-39095}"
SOLARIS_CLIENT_MCP_GAME_DIR="${SOLARIS_CLIENT_MCP_GAME_DIR:-$AGENT_ROOT/fabric-agent/run-mcp}"
SOLARIS_CLIENT_MCP_USERNAME="${SOLARIS_CLIENT_MCP_USERNAME:-SolarisMcp}"

if [[ ! "$SOLARIS_CLIENT_MCP_PORT" =~ ^[0-9]{1,5}$ ]]; then
    printf 'Invalid SOLARIS_CLIENT_MCP_PORT: use an integer from 1 to 65535.\n' >&2
    exit 2
fi
SOLARIS_CLIENT_MCP_PORT=$((10#$SOLARIS_CLIENT_MCP_PORT))
if (( SOLARIS_CLIENT_MCP_PORT < 1 || SOLARIS_CLIENT_MCP_PORT > 65535 )); then
    printf 'Invalid SOLARIS_CLIENT_MCP_PORT: use an integer from 1 to 65535.\n' >&2
    exit 2
fi

if [[ ! "$SOLARIS_CLIENT_MCP_USERNAME" =~ ^[A-Za-z0-9_]{1,16}$ ]]; then
    printf 'Invalid SOLARIS_CLIENT_MCP_USERNAME: use 1..16 ASCII letters, digits, or underscores.\n' >&2
    exit 2
fi

if [[ "$SOLARIS_CLIENT_MCP_GAME_DIR" != /* ]]; then
    SOLARIS_CLIENT_MCP_GAME_DIR="$REPO_ROOT/$SOLARIS_CLIENT_MCP_GAME_DIR"
fi

export SOLARIS_CLIENT_MCP_PORT

printf 'Minecraft MCP endpoint: http://127.0.0.1:%s/mcp\n' "$SOLARIS_CLIENT_MCP_PORT"
printf 'Minecraft game directory: %s\n' "$SOLARIS_CLIENT_MCP_GAME_DIR"

if [[ ! -x "$AGENT_ROOT/gradlew" ]]; then
    printf 'Missing executable Gradle wrapper: %s\n' "$AGENT_ROOT/gradlew" >&2
    exit 1
fi

if [[ "$MODE" == "check" ]]; then
    if ! command -v java >/dev/null 2>&1; then
        printf 'Java 25 is required but java is not available.\n' >&2
        exit 1
    fi
    java_specification_version="$({ java -XshowSettings:properties -version; } 2>&1 \
        | sed -n 's/^[[:space:]]*java\.specification\.version = //p' \
        | head -n 1)"
    if [[ "$java_specification_version" != "25" ]]; then
        printf 'Java 25 is required; found specification version %s.\n' \
            "${java_specification_version:-unknown}" >&2
        exit 1
    fi

    "$AGENT_ROOT/gradlew" \
        --no-configuration-cache \
        -p "$AGENT_ROOT" \
        "-Psolaris.clientMcp.gameDir=$SOLARIS_CLIENT_MCP_GAME_DIR" \
        "-Psolaris.clientMcp.username=$SOLARIS_CLIENT_MCP_USERNAME" \
        :fabric-agent:validateClientMcpRunProperties \
        :bridge-core:test \
        --tests dev.solaris.agent.mcp.McpHttpServerTest
    printf 'Minecraft MCP check passed; client was not launched.\n'
    exit 0
fi

if (exec 3<>"/dev/tcp/127.0.0.1/$SOLARIS_CLIENT_MCP_PORT") 2>/dev/null; then
    printf 'MCP port %s is already in use; stop the existing client or choose another port.\n' \
        "$SOLARIS_CLIENT_MCP_PORT" >&2
    exit 1
fi

exec "$AGENT_ROOT/gradlew" \
    --no-configuration-cache \
    -p "$AGENT_ROOT" \
    "-Psolaris.clientMcp.gameDir=$SOLARIS_CLIENT_MCP_GAME_DIR" \
    "-Psolaris.clientMcp.username=$SOLARIS_CLIENT_MCP_USERNAME" \
    :fabric-agent:runClientMcp
