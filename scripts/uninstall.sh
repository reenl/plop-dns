#!/usr/bin/env bash
set -euo pipefail
sudo systemctl disable --now docker-dns.service
sudo rm -f /etc/systemd/system/docker-dns.service /usr/local/bin/docker-dns
sudo rm -f /etc/systemd/system/docker-dns.service.d/30-docker-domains.conf
sudo systemctl daemon-reload
if [[ -f /etc/systemd/dns-delegate.d/30-docker-domains.dns-delegate ]]; then
  sudo rm -f /etc/systemd/dns-delegate.d/30-docker-domains.dns-delegate
  sudo systemctl reload systemd-resolved.service
fi
# Leaves Omarchy DNS configuration and Docker resources intact.
