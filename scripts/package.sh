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
binary="target/$target/release/docker-dns"
test -x "$binary" || { echo "Build first: cargo build --release --locked --target $target" >&2; exit 1; }
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml)
name="docker-dns-$version-linux-$architecture"
staging=$(mktemp -d)
trap 'rm -rf -- "$staging"' EXIT
mkdir -p "$staging/$name/scripts" target/dist
install -m 755 "$binary" "$staging/$name/docker-dns"
install -m 755 scripts/install.sh scripts/uninstall.sh "$staging/$name/scripts/"
cp -R systemd "$staging/$name/"
cp -R docs "$staging/$name/"
cp README.md CONTRIBUTING.md LICENSE "$staging/$name/"
tar -czf "target/dist/$name.tar.gz" -C "$staging" "$name"
echo "target/dist/$name.tar.gz"
