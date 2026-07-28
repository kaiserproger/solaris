#!/usr/bin/env bash
set -euo pipefail

repository="${SOLARIS_REPOSITORY:-kaiserproger/solaris}"
version="${SOLARIS_VERSION:-latest}"
binary_name="${SOLARIS_BINARY_NAME:-solaris}"

fail() {
  printf 'solaris installer: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

need curl
need tar
need install
need uname

os="$(uname -s)"
arch="$(uname -m)"

if [[ -n "${SOLARIS_TARGET:-}" ]]; then
  target="$SOLARIS_TARGET"
else
  case "$os" in
    Linux) ;;
    *) fail "unsupported operating system: $os (tagged binaries currently support Linux only)" ;;
  esac

  case "$arch" in
    x86_64|amd64) target="x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) target="aarch64-unknown-linux-gnu" ;;
    *) fail "unsupported architecture: $arch" ;;
  esac
fi

asset="solaris-${target}.tar.gz"
checksum_asset="${asset}.sha256"

if [[ -n "${SOLARIS_DOWNLOAD_BASE:-}" ]]; then
  download_base="${SOLARIS_DOWNLOAD_BASE%/}"
elif [[ "$version" == "latest" ]]; then
  download_base="https://github.com/${repository}/releases/latest/download"
else
  [[ "$version" == v* ]] || version="v${version}"
  download_base="https://github.com/${repository}/releases/download/${version}"
fi

if [[ -n "${SOLARIS_INSTALL_DIR:-}" ]]; then
  install_dir="$SOLARIS_INSTALL_DIR"
elif [[ "$(id -u)" -eq 0 ]]; then
  install_dir="/usr/local/bin"
else
  [[ -n "${HOME:-}" ]] || fail "HOME is not set; provide SOLARIS_INSTALL_DIR"
  install_dir="$HOME/.local/bin"
fi

tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t solaris-install)"
trap 'rm -rf "$tmp_dir"' EXIT
archive="$tmp_dir/$asset"
checksum_file="$tmp_dir/$checksum_asset"
unpack_dir="$tmp_dir/unpack"
mkdir -p "$unpack_dir"

curl --fail --silent --show-error --location --retry 3 \
  --output "$archive" "$download_base/$asset"
curl --fail --silent --show-error --location --retry 3 \
  --output "$checksum_file" "$download_base/$checksum_asset"

expected="$(awk 'NR == 1 { print $1 }' "$checksum_file")"
[[ "$expected" =~ ^[0-9a-fA-F]{64}$ ]] || fail "invalid SHA-256 file for $asset"

if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$archive" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$archive" | awk '{ print $1 }')"
else
  fail "sha256sum or shasum is required to verify the release"
fi

[[ "${actual,,}" == "${expected,,}" ]] || fail "SHA-256 mismatch for $asset"

if tar -tvzf "$archive" | awk '
  substr($1, 1, 1) == "l" || substr($1, 1, 1) == "h" { found = 1 }
  END { exit found ? 0 : 1 }
'; then
  fail "release archive contains a symbolic or hard link"
fi

declare -A seen_entries=()
binary_entries=0
while IFS= read -r entry; do
  case "$entry" in
    solaris|README.md|example.toml|LICENSE-APACHE|LICENSE-MIT|VERSION) ;;
    /*|../*|*/../*|*/..) fail "unsafe path in release archive: $entry" ;;
    *) fail "unexpected path in release archive: $entry" ;;
  esac
  [[ -z "${seen_entries[$entry]+present}" ]] || fail "duplicate path in release archive: $entry"
  seen_entries[$entry]=1
  [[ "$entry" == "solaris" ]] && binary_entries=$((binary_entries + 1))
done < <(tar -tzf "$archive")
[[ "$binary_entries" -eq 1 ]] || fail "release archive must contain exactly one solaris binary"

tar -xzf "$archive" -C "$unpack_dir"
[[ -f "$unpack_dir/solaris" ]] || fail "release archive did not extract a solaris file"

mkdir -p "$install_dir"
install -m 0755 "$unpack_dir/solaris" "$install_dir/$binary_name"

printf 'Solaris installed: %s\n' "$install_dir/$binary_name"
case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *) printf 'Add %s to PATH to run it as `%s`.\n' "$install_dir" "$binary_name" ;;
esac
