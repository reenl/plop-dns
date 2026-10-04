#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
if [[ $# != 1 ]]; then
  echo "Usage: $0 <x86_64-unknown-linux-musl|aarch64-unknown-linux-musl>" >&2
  exit 1
fi
case "$1" in
  x86_64-unknown-linux-musl) architecture=x86_64 ;;
  aarch64-unknown-linux-musl) architecture=aarch64 ;;
  *) echo "Unsupported release target: $1" >&2; exit 1 ;;
esac
binary="target/$1/release/docker-dns"
test -x "$binary" || { echo "Build first: cargo build --release --locked --target $1" >&2; exit 1; }
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml)
name="docker-dns-v$version-linux-$architecture"
staging=$(mktemp -d)
trap 'rm -rf -- "$staging"' EXIT
mkdir -p "$staging/$name/scripts" target/dist
install -m 755 "$binary" "$staging/$name/docker-dns"
install -m 755 scripts/install.sh scripts/uninstall.sh "$staging/$name/scripts/"
cp -R systemd "$staging/$name/"
cp README.md LICENSE "$staging/$name/"
tar -czf "target/dist/$name.tar.gz" -C "$staging" "$name"
echo "target/dist/$name.tar.gz"
