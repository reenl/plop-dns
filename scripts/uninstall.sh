#!/usr/bin/env bash
set -euo pipefail
sudo systemctl disable --now plop-dns.service
sudo rm -f /etc/systemd/system/plop-dns.service /usr/local/bin/plop-dns
sudo systemctl daemon-reload
if [[ -f /etc/systemd/dns-delegate.d/30-docker-domains.dns-delegate ]]; then
  sudo rm -f /etc/systemd/dns-delegate.d/30-docker-domains.dns-delegate
  sudo systemctl reload systemd-resolved.service
fi
# Leaves Omarchy DNS configuration and Docker resources intact.
