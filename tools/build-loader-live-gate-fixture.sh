#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
FIXTURE_ROOT="$REPO_ROOT/examples/loader-live-gate"
PLUGIN_ROOT="$FIXTURE_ROOT/plugins"
DEPLOY_ROOT="$REPO_ROOT/.analysis/loader-live-gate/plugins"
GUEST_MANIFEST="$REPO_ROOT/sdk/rust/Cargo.toml"
GUEST_MODULE="$REPO_ROOT/sdk/rust/target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm"
MODE="build"

usage() {
    printf '%s\n' \
        'Usage: tools/build-loader-live-gate-fixture.sh [--check|--prepare|--audio]' \
        '' \
        '  (default)   regenerate the authored client bundles and package manifests' \
        '  --check     verify the authored outputs are exactly reproducible from source' \
        '  --prepare   verify the fixture, build the real SDK guest and stage the' \
        '              deployable packages under .analysis/loader-live-gate/plugins' \
        '  --audio     regenerate the tracked OGG tone sources (needs ffmpeg+libvorbis)'
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
        --prepare)
            MODE="prepare"
            ;;
        --audio)
            MODE="audio"
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

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        printf 'Missing fixture build command: %s\n' "$1" >&2
        exit 1
    fi
}

for fixture_command in sed stat sha256sum cmp python3 zip zipinfo; do
    require_command "$fixture_command"
done

sha256() {
    sha256sum "$1" | sed 's/[[:space:]].*$//'
}

# One fixture owner per row: its id, the item/block names its bundle declares,
# the tone frequency the audio phase measures, and the Ogg page serial the
# normalizer pins. One table, so a third owner is a row and not a second script.
owners() {
    printf '%s %s %s %s\n' ruby-live ruby 440 1105
    printf '%s %s %s %s\n' sapphire-live sapphire 660 1106
}

require_asset() {
    if [[ ! -f "$1" ]]; then
        printf 'Fixture source is missing: %s\n' "${1#"$REPO_ROOT"/}" >&2
        exit 1
    fi
}

# ---------------------------------------------------------------------------
# Audio sources
#
# The two tones are the gate's measurement inputs: the audio phase reads a real
# 48 kHz capture and requires a 440 Hz Ruby tone and a 660 Hz Sapphire tone. They
# are authored sources, tracked next to the JSON assets and regenerated
# byte-for-byte by `--audio`: ffmpeg synthesizes a mono 8 second sine and the
# normalizer rewrites the Ogg page serial and checksum, which are the only bytes
# ffmpeg leaves to chance.
# ---------------------------------------------------------------------------
normalize_ogg() {
    python3 - "$1" "$2" <<'PYTHON'
import struct
import sys

POLYNOMIAL = 0x04C11DB7
TABLE = []
for index in range(256):
    value = index << 24
    for _ in range(8):
        value = (
            ((value << 1) ^ POLYNOMIAL) & 0xFFFFFFFF
            if value & 0x80000000
            else (value << 1) & 0xFFFFFFFF
        )
    TABLE.append(value)


def crc32(data):
    state = 0
    for byte in data:
        state = ((state << 8) & 0xFFFFFFFF) ^ TABLE[((state >> 24) & 0xFF) ^ byte]
    return state


path = sys.argv[1]
serial = int(sys.argv[2])
raw = bytearray(open(path, "rb").read())
offset = 0
pages = 0
while offset < len(raw):
    if raw[offset:offset + 4] != b"OggS":
        raise SystemExit(f"{path}: page {pages} does not start with OggS")
    segments = raw[offset + 26]
    body = offset + 27 + segments
    end = body + sum(raw[offset + 27:body])
    struct.pack_into("<I", raw, offset + 14, serial)
    struct.pack_into("<I", raw, offset + 22, 0)
    struct.pack_into("<I", raw, offset + 22, crc32(bytes(raw[offset:end])))
    offset = end
    pages += 1
open(path, "wb").write(bytes(raw))
PYTHON
}

build_audio() {
    local owner frequency serial path
    while read -r owner _item_name frequency serial; do
        require_command ffmpeg
        path="$PLUGIN_ROOT/$owner/client-src/assets/$owner/sounds/tone.ogg"
        mkdir -p "$(dirname -- "$path")"
        ffmpeg -nostdin -hide_banner -loglevel error -y \
            -f lavfi -i "sine=frequency=$frequency:sample_rate=48000:duration=8" \
            -ac 1 -map_metadata -1 -c:a libvorbis -q:a 3 "$path" < /dev/null
        normalize_ogg "$path" "$serial"
        printf 'Wrote %s.\n' "${path#"$REPO_ROOT"/}"
    done < <(owners)
}

# ---------------------------------------------------------------------------
# Authored outputs: one client bundle per owner and the package manifest that
# pins it. `--check` and `--prepare` compare instead of writing, so a prepared
# deployment is exactly what the comparison already proved reproducible.
# ---------------------------------------------------------------------------
build_owner() {
    local owner="$1"
    local item_name="$2"
    local plugin_root="$PLUGIN_ROOT/$owner"
    local source_root="$plugin_root/client-src"
    local stage_root="$FIXTURE_TMP/$owner-stage"
    local item_path="assets/$owner/items/$item_name.json"
    local block_path="assets/$owner/models/block/${item_name}_block.json"
    local tone_path="assets/$owner/sounds/tone.ogg"
    local archive_output="$FIXTURE_TMP/$owner-rich-content.zip"
    local manifest_output="$FIXTURE_TMP/$owner-plugin.toml"

    require_asset "$source_root/solaris-client.json.in"
    require_asset "$source_root/$item_path"
    require_asset "$source_root/$block_path"
    require_asset "$source_root/$tone_path"
    require_asset "$plugin_root/plugin.toml.in"

    mkdir -p "$stage_root"
    cp -R "$source_root/assets" "$stage_root/assets"

    sed \
        -e "s/@ITEM_SHA256@/$(sha256 "$source_root/$item_path")/g" \
        -e "s/@ITEM_SIZE@/$(stat -c %s "$source_root/$item_path")/g" \
        -e "s/@BLOCK_SHA256@/$(sha256 "$source_root/$block_path")/g" \
        -e "s/@BLOCK_SIZE@/$(stat -c %s "$source_root/$block_path")/g" \
        -e "s/@TONE_SHA256@/$(sha256 "$source_root/$tone_path")/g" \
        -e "s/@TONE_SIZE@/$(stat -c %s "$source_root/$tone_path")/g" \
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

    if [[ "$MODE" != "build" ]]; then
        cmp "$archive_output" "$plugin_root/client/rich-content.zip"
        cmp "$manifest_output" "$plugin_root/plugin.toml"
        return
    fi

    mkdir -p "$plugin_root/client"
    install -m 0644 "$archive_output" "$plugin_root/client/rich-content.zip"
    install -m 0644 "$manifest_output" "$plugin_root/plugin.toml"
}

# ---------------------------------------------------------------------------
# Deployment preparation: the tracked directory is the package, and the one piece
# a source tree cannot carry is the built component. `--prepare` compiles the SDK
# guest, encodes it into a component exactly as a published package is built, and
# stages a complete package - manifest, component, configuration and verified
# client bundle - under the ignored deployment root. Nothing here runs the guest
# or claims a gameplay result: the Loader gate is the harness scenario that
# deploys this directory.
# ---------------------------------------------------------------------------
prepare_guest() {
    require_command cargo

    (cd "$REPO_ROOT" && cargo build \
        --manifest-path "$GUEST_MANIFEST" \
        --target wasm32-unknown-unknown \
        --release \
        -p solaris-hello-plugin)
    if [[ ! -f "$GUEST_MODULE" ]]; then
        printf 'The guest build did not produce %s.\n' "${GUEST_MODULE#"$REPO_ROOT"/}" >&2
        exit 1
    fi
    (cd "$REPO_ROOT" && cargo run --quiet \
        --manifest-path "$REPO_ROOT/crates/mc-plugin-host/Cargo.toml" \
        --example plugin_component -- "$GUEST_MODULE" "$FIXTURE_TMP/plugin.wasm")
}

prepare_owner() {
    local owner="$1"
    local plugin_root="$PLUGIN_ROOT/$owner"
    local deploy_root="$DEPLOY_ROOT/$owner"

    require_asset "$plugin_root/plugin.toml"
    require_asset "$plugin_root/config.toml"
    require_asset "$plugin_root/client/rich-content.zip"

    rm -rf "$deploy_root"
    mkdir -p "$deploy_root/client"
    install -m 0644 "$plugin_root/plugin.toml" "$deploy_root/plugin.toml"
    install -m 0644 "$plugin_root/config.toml" "$deploy_root/config.toml"
    install -m 0644 "$plugin_root/client/rich-content.zip" "$deploy_root/client/rich-content.zip"
    install -m 0644 "$FIXTURE_TMP/plugin.wasm" "$deploy_root/plugin.wasm"
    printf 'Staged %s.\n' "${deploy_root#"$REPO_ROOT"/}"
}

if [[ "$MODE" == "audio" ]]; then
    build_audio
    printf 'Regenerated the tracked Loader live-gate tones.\n'
    exit 0
fi

FIXTURE_TMP="$(mktemp -d)"
cleanup() {
    rm -rf -- "$FIXTURE_TMP"
}
trap cleanup EXIT

while read -r owner item_name _frequency _serial; do
    build_owner "$owner" "$item_name"
done < <(owners)

if [[ "$MODE" == "prepare" ]]; then
    prepare_guest
    while read -r owner _item_name _frequency _serial; do
        prepare_owner "$owner"
    done < <(owners)
fi

case "$MODE" in
    check)
        printf 'Loader live-gate fixture is reproducible and current.\n'
        ;;
    prepare)
        printf 'Prepared the Loader live-gate deployment under %s.\n' "${DEPLOY_ROOT#"$REPO_ROOT"/}"
        ;;
    *)
        printf 'Built Loader live-gate fixture under %s.\n' "${PLUGIN_ROOT#"$REPO_ROOT"/}"
        ;;
esac
