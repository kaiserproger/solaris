#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t solaris-installer-test)"
trap 'rm -rf "$tmp_dir"' EXIT

assets="$tmp_dir/assets"
package="$tmp_dir/package"
install_dir="$tmp_dir/bin"
target="x86_64-unknown-linux-gnu"
asset="solaris-${target}.tar.gz"
mkdir -p "$assets" "$package" "$install_dir"

cat > "$package/solaris" <<'EOF'
#!/usr/bin/env sh
printf 'solaris-installer-fixture\n'
EOF
chmod 0755 "$package/solaris"

tar -C "$package" -czf "$assets/$asset" solaris
(
  cd "$assets"
  sha256sum "$asset" > "${asset}.sha256"
)

bash -n "$repo_root/install.sh"

SOLARIS_DOWNLOAD_BASE="file://$assets" \
SOLARIS_INSTALL_DIR="$install_dir" \
SOLARIS_TARGET="$target" \
  bash "$repo_root/install.sh"

[[ -x "$install_dir/solaris" ]]
[[ "$($install_dir/solaris)" == "solaris-installer-fixture" ]]

installed_sha="$(sha256sum "$install_dir/solaris" | awk '{ print $1 }')"
printf '%064d  %s\n' 0 "$asset" > "$assets/${asset}.sha256"

if SOLARIS_DOWNLOAD_BASE="file://$assets" \
   SOLARIS_INSTALL_DIR="$install_dir" \
   SOLARIS_TARGET="$target" \
     bash "$repo_root/install.sh" >"$tmp_dir/mismatch.out" 2>"$tmp_dir/mismatch.err"; then
  echo "installer unexpectedly accepted a mismatched checksum" >&2
  exit 1
fi

grep -q "SHA-256 mismatch" "$tmp_dir/mismatch.err"
[[ "$(sha256sum "$install_dir/solaris" | awk '{ print $1 }')" == "$installed_sha" ]]

rm -rf "$package"
mkdir -p "$package"
ln -s /bin/sh "$package/solaris"
tar -C "$package" -czf "$assets/$asset" solaris
(
  cd "$assets"
  sha256sum "$asset" > "${asset}.sha256"
)
if SOLARIS_DOWNLOAD_BASE="file://$assets" \
   SOLARIS_INSTALL_DIR="$install_dir" \
   SOLARIS_TARGET="$target" \
     bash "$repo_root/install.sh" >"$tmp_dir/link.out" 2>"$tmp_dir/link.err"; then
  echo "installer unexpectedly accepted a symbolic link archive" >&2
  exit 1
fi
grep -q "symbolic or hard link" "$tmp_dir/link.err"
[[ "$(sha256sum "$install_dir/solaris" | awk '{ print $1 }')" == "$installed_sha" ]]

echo "installer tests passed"
