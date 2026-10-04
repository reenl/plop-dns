#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
binary=./plop-dns
if [[ ! -x "$binary" ]]; then
  binary=target/release/plop-dns
fi
test -x "$binary" || { echo 'Extract a release archive or run cargo build --release --locked first.' >&2; exit 1; }
systemd_version=$(systemctl --version | sed -n '1s/^systemd \([0-9]*\).*/\1/p')
if [[ ! "$systemd_version" =~ ^[0-9]+$ ]] || (( systemd_version < 258 )); then
  echo 'plop-dns requires systemd 258 or newer for DNS delegation.' >&2
  exit 1
fi
systemctl is-active --quiet systemd-resolved.service || { echo 'Enable systemd-resolved before installing plop-dns.' >&2; exit 1; }
sudo install -m 755 "$binary" /usr/local/bin/plop-dns
sudo install -m 644 systemd/system/plop-dns.service /etc/systemd/system/plop-dns.service
sudo install -d -m 755 /etc/systemd/dns-delegate.d
sudo install -m 644 systemd/dns-delegate.d/30-docker-domains.dns-delegate /etc/systemd/dns-delegate.d/30-docker-domains.dns-delegate
# Remove the old loopback routing drop-in when upgrading.
sudo rm -f /etc/systemd/system/docker-dns.service.d/30-docker-domains.conf
sudo systemctl daemon-reload
# Enable under docker.service, never under multi-user.target. Do not use --now.
sudo systemctl enable plop-dns.service
sudo systemctl reload systemd-resolved.service
if systemctl is-active --quiet docker.service; then
  sudo systemctl restart plop-dns.service
fi
