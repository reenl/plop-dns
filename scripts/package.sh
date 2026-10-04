#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
if [[ $# -gt 1 ]]; then
  echo "Usage: $0 [x86_64|aarch64]" >&2
  exit 1
fi
architecture=${1:-$(uname -m)}
case "$architecture" in
  x86_64|aarch64) ;;
  *) echo "Unsupported architecture: $architecture" >&2; exit 1 ;;
esac
target="$architecture-unknown-linux-musl"
binary="target/$target/release/plop-dns"
test -x "$binary" || { echo "Build first: cargo build --release --locked --target $target" >&2; exit 1; }
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml)
name="plop-dns-$version-linux-$architecture"
staging=$(mktemp -d)
trap 'rm -rf -- "$staging"' EXIT
mkdir -p "$staging/$name" target/dist
install -m 755 "$binary" "$staging/$name/plop-dns"
install -m 755 scripts/install.sh scripts/uninstall.sh "$staging/$name/"
cp LICENSE "$staging/$name/"
cargo about generate --locked --fail --target "$target" \
  -o "$staging/$name/THIRD-PARTY-LICENSES.txt" about.hbs
# Preserve separate attribution notices alongside the generated license texts.
cargo metadata --locked --format-version 1 --filter-platform "$target" > "$staging/metadata.json"
jq -r '.packages[] | select(.source != null) | .manifest_path' "$staging/metadata.json" > "$staging/manifests"
while IFS= read -r manifest; do
  directory=$(dirname -- "$manifest")
  find "$directory" -type f \( -iname 'NOTICE*' -o -iname 'COPYRIGHT*' \) -print > "$staging/notices"
  while IFS= read -r notice; do
    printf '\n=== %s / %s ===\n\n' "$(basename -- "$directory")" "${notice#"$directory"/}"
    cat -- "$notice"
  done < "$staging/notices"
done < "$staging/manifests" >> "$staging/$name/THIRD-PARTY-LICENSES.txt"
tar -czf "target/dist/$name.tar.gz" -C "$staging" "$name"
echo "target/dist/$name.tar.gz"
