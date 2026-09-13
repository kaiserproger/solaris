#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
FIXTURE_ROOT="$REPO_ROOT/examples/loader-live-gate/plugins"
MODE="build"

usage() {
    printf '%s\n' 'Usage: tools/build-loader-live-gate-fixture.sh [--check]'
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

for fixture_command in zip zipinfo sha256sum stat sed cmp; do
    if ! command -v "$fixture_command" >/dev/null 2>&1; then
        printf 'Missing fixture build command: %s\n' "$fixture_command" >&2
        exit 1
    fi
done

FIXTURE_TMP="$(mktemp -d)"
cleanup() {
    rm -rf -- "$FIXTURE_TMP"
}
trap cleanup EXIT

sha256() {
    sha256sum "$1" | sed 's/[[:space:]].*$//'
}

build_owner() {
    local owner="$1"
    local item_name="$2"
    local plugin_root="$FIXTURE_ROOT/$owner"
    local source_root="$plugin_root/client-src"
    local stage_root="$FIXTURE_TMP/$owner-stage"
    local item_path="assets/$owner/items/$item_name.json"
    local block_path="assets/$owner/models/block/${item_name}_block.json"
    local archive_output="$FIXTURE_TMP/$owner-rich-content.zip"
    local manifest_output="$FIXTURE_TMP/$owner-plugin.toml"

    mkdir -p "$stage_root"
    cp -R "$source_root/assets" "$stage_root/assets"

    sed \
        -e "s/@ITEM_SHA256@/$(sha256 "$source_root/$item_path")/g" \
        -e "s/@ITEM_SIZE@/$(stat -c %s "$source_root/$item_path")/g" \
        -e "s/@BLOCK_SHA256@/$(sha256 "$source_root/$block_path")/g" \
        -e "s/@BLOCK_SIZE@/$(stat -c %s "$source_root/$block_path")/g" \
        "$source_root/solaris-client.json.in" > "$stage_root/solaris-client.json"

    find "$stage_root" -type f -exec chmod 0644 {} +
    find "$stage_root" -type f -exec touch -t 200001010000 {} +
    mapfile -t archive_files < <(
        cd "$stage_root"
        find assets -type f -print | LC_ALL=C sort
    )
    (
        cd "$stage_root"
        zip -X -q "$archive_output" solaris-client.json "${archive_files[@]}"
    )

    local first_entry
    first_entry="$(zipinfo -1 "$archive_output" | sed -n '1p')"
    if [[ "$first_entry" != "solaris-client.json" ]]; then
        printf '%s archive does not begin with solaris-client.json.\n' "$owner" >&2
        exit 1
    fi

    sed \
        -e "s/@ARTIFACT_SHA256@/$(sha256 "$archive_output")/g" \
        -e "s/@ARTIFACT_SIZE@/$(stat -c %s "$archive_output")/g" \
        "$plugin_root/plugin.toml.in" > "$manifest_output"

    if [[ "$MODE" == "check" ]]; then
        cmp "$archive_output" "$plugin_root/client/rich-content.zip"
        cmp "$manifest_output" "$plugin_root/plugin.toml"
        return
    fi

    mkdir -p "$plugin_root/client"
    install -m 0644 "$archive_output" "$plugin_root/client/rich-content.zip"
    install -m 0644 "$manifest_output" "$plugin_root/plugin.toml"
}

build_owner "ruby-live" "ruby"
build_owner "sapphire-live" "sapphire"

if [[ "$MODE" == "check" ]]; then
    printf 'Loader live-gate fixture is reproducible and current.\n'
else
    printf 'Built Loader live-gate fixture under %s.\n' "$FIXTURE_ROOT"
fi
