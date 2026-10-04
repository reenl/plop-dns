# docker-dns

A native Rust service giving running Compose containers these records:

```text
web.myproject.docker      all web replica IPs, rotating the first answer
web-1.myproject.docker    replica 1
web-2.myproject.docker    replica 2
```

The suffix is `.docker`, listener `127.0.0.1:5354`, and TTL 0 (no DNS answer caching).
These are fixed constants. There is no CLI configuration, forwarding,
upstream resolver, network filter, network creation or container modification.

Compose labels determine the names. Discovery takes one snapshot on connection,
then refreshes only for container start/exit/removal and network attachment events.
Immediately available events are drained before taking a snapshot. Events still
arriving can trigger the next refresh.
An idle Docker daemon receives no periodic requests. After an event-stream failure,
discovery refreshes the snapshot and reconnects after two seconds using a new client.
If refresh also fails or Docker is inactive, records are cleared. Errors are logged
from the third consecutive failure; a successfully handled event resets the count.
Reconnection takes a fresh snapshot. Events since the snapshot started are replayed to cover startup changes.
It selects each container's network using `dns.network` or network priorities,
with deterministic tie breaking. One-off Compose
run containers and invalid hostname labels are skipped. UDP/TCP and A/AAAA are
supported; missing local names return NXDOMAIN, unavailable discovery SERVFAIL,
and names outside `.docker` REFUSED.

## Compose labels

```yaml
labels:
  dns: special.docker
  dns.default: "true"
  dns.network: workspace-services
```

`dns` adds the given name alongside the automatic service and replica records.
`dns: special` also registers `special.docker`. `dns.default: "true"` additionally
registers `<compose_project_name>.docker`. Quote the boolean value in Compose.
Without `dns.default`, the project name exists with no addresses and returns an
empty successful answer. It disappears when its last registered service disappears.

Containers registering the same name contribute their addresses to one record;
queries rotate the first address. Duplicate addresses are removed. Aliases follow
container start/stop just like automatic records, and all records have TTL 0.

`dns.network` selects the attached Docker network used for all of that container's
records, including IPv4 and IPv6. Use its actual Docker network name (Compose may
prefix it with the project name). If that network is not attached, the container
is skipped and the reason is logged.

Set `dns.priority` on the Docker network to influence selection for containers
without `dns.network`. Lower values win; the default is 500. For example:

```yaml
networks:
  shared:
    labels:
      dns.priority: "1"
  background:
    labels:
      dns.priority: "999"
```

With equal priorities, the primary network wins, then alphabetical network name.
Priorities are unsigned integers; invalid values are logged and treated as 500.
Each snapshot reads network labels once, alongside the running container list.

## Install

Requirements:

- Linux on x86_64 or ARM64 (`aarch64`). Release binaries are statically linked.
- Rootful Docker managed by `docker.service`, using the local Unix socket.
- Active systemd-resolved, with systemd 258 or newer for DNS delegation.
- Host DNS configured to use resolved (through NSS or its resolver stub).
- Bash and sudo for the install/uninstall scripts.

### From a release

Download the archive matching `uname -m` and `SHA256SUMS` from this repository's
GitHub Releases page. No Rust installation is needed. For example, on x86_64:

```bash
sha256sum --ignore-missing --check SHA256SUMS
tar -xzf docker-dns-v0.1.0-linux-x86_64.tar.gz
cd docker-dns-v0.1.0-linux-x86_64
./scripts/install.sh
```

On ARM64, use `docker-dns-v0.1.0-linux-aarch64.tar.gz` instead. Each archive includes
the binary, systemd configuration, install/uninstall scripts, this guide, and MIT
license. Keep the extracted directory to run uninstall later. To upgrade, extract
the new release and run its installer.

### From source

```bash
cargo build --release --locked
./scripts/install.sh
```

Repository configuration paths mirror their destinations under `/etc`:

```text
systemd/system/docker-dns.service
systemd/dns-delegate.d/30-docker-domains.dns-delegate
```

The DNS delegation file routes `.docker` to `127.0.0.1:5354`, with
`DefaultRoute=no`. Ordinary DNS continues through existing upstreams. Installation
and uninstall reload resolved to apply or remove the delegation.

### DNS inside containers

On Omarchy, container DNS already goes through host resolved at `172.17.0.1:53`.
No Docker DNS changes or Compose `dns:` settings are needed there.

On other distributions, host DNS delegation alone does not guarantee Docker
forwards queries through resolved. Check a container's `/etc/resolv.conf` and test
`nslookup web.myproject.docker`. A nameserver of `127.0.0.11` is Docker's embedded
resolver; its upstream still needs to reach host resolved.

If that forwarding is not already configured, the following matches Omarchy's
setup for a default rootful bridge at `172.17.0.1`:

```ini
# /etc/systemd/resolved.conf.d/20-docker-dns.conf
[Resolve]
DNSStubListenerExtra=172.17.0.1
```

Merge this property into `/etc/docker/daemon.json`, preserving existing settings:

```json
{"dns": ["172.17.0.1"]}
```

Use your actual host bridge address if it differs. Once the bridge exists, apply
these settings with:

```bash
sudo systemctl restart docker.service
sudo systemctl restart systemd-resolved.service
```

Restarting Docker may interrupt running containers. Host firewall rules must allow containers to reach
that address on UDP/TCP port 53. These host-wide settings are separate from this
project: its installer and uninstaller do not modify them.

```text
host      -> resolved -> .docker -> docker-dns
container -> Docker DNS -> resolved -> .docker -> docker-dns
```

Delegation provides a domain-specific DNS scope without configuring an interface.

Installation does not wake Docker. The unit is enabled under `docker.service`,
with no dependency that starts Docker. `ExecCondition` skips a manual start when
Docker is inactive; the binary checks service state before discovery. DNS queries
only read in-memory records. Systemd stops DNS before Docker. A manually run binary's
state check is not atomic against concurrent Docker shutdown; use the unit.

DNS does not create connectivity between isolated networks. Containers connecting
to shared Postgres/Redis/S3 need a common network or a working route. The host can
reach rootful Docker bridge addresses directly.

## Check or uninstall

```bash
resolvectl query web.myproject.docker
dig @127.0.0.1 -p 5354 web.myproject.docker
systemctl status docker-dns.service
journalctl -u docker-dns.service -n 50 --no-pager
./scripts/uninstall.sh
```

Replace the example name with a running Compose service. In an Alpine container,
use `nslookup web.myproject.docker` to test its normal DNS path. Docker must be
active for the daemon to run; installation leaves inactive Docker asleep.

Uninstall stops and disables the daemon, removes its binary, unit and delegation
file, then reloads resolved. It preserves Docker containers/networks and existing
host DNS settings.
## Dependencies and tests

| Dependency | Purpose |
| --- | --- |
| `bollard` | Docker Unix-socket API: version negotiation, container list and events. Only its socket transport is enabled. |
| `hickory-proto` | Parse and encode DNS messages, including compressed names and records. Its network runtime is disabled. |
| `tokio` | Async UDP/TCP, Docker I/O, timers, service-state subprocess and task coordination. Only used features are enabled. |
| `futures-util` | Read Docker's async event stream with `.next()`. |
| `anyhow` | Propagate and report different I/O, Docker and DNS errors without a custom error hierarchy. |

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Unit tests live in the test-only module at the bottom of `src/main.rs`.
`tests/lazy_start.rs` checks the running binary; `tests/fixtures/` holds fake Docker data. They cover
records, replica rotation, Docker fixture discovery/removal, DNS responses and
transport. The process test verifies that inactive Docker is never contacted,
active idle Docker is not polled, and a container event triggers a refresh.
It also checks that a queued burst produces one refresh, and checks snapshot
refresh and reconnection after a broken event stream.
It runs the actual binary against a fake Docker socket and requires port
`127.0.0.1:5354` to be free. Real Docker is never queried by the tests.

## Releases

The GitHub workflow builds and tests both architectures on native runners, using
musl for static binaries. Branch pushes, pull requests, and manual workflow runs
produce downloadable Actions artifacts. No local ARM compiler is needed.

To release, set the version in `Cargo.toml`, update `Cargo.lock` with `cargo check`,
commit the changes, and push the matching tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Only tags matching the package version create a draft GitHub release. After both
builds and tests pass, it contains both archives and `SHA256SUMS`. Review the draft
and publish it from GitHub Releases. No personal access token is required by the
workflow; it uses GitHub's repository token.

To package an already built static binary locally:

```bash
cargo build --release --locked --target x86_64-unknown-linux-musl
./scripts/package.sh x86_64-unknown-linux-musl
```

This requires the target's Rust standard library and a suitable musl linker.
Archives are written under `target/dist/`. The ARM64 target is
`aarch64-unknown-linux-musl`.

Licensed under the [MIT license](LICENSE).
