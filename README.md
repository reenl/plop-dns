# plop-dns

Automatically create DNS records for your Docker Compose containers so you can
reach them without publishing ports when using Linux bridge networking. Run the
same project multiple times: each gets its own names, and services keep their
normal ports.

This targets Linux with systemd and systemd-resolved. It assumes containers can
query host resolved through `DNSStubListenerExtra`, as configured by Omarchy.
Setup for other distributions is covered below.

## What it is and what it is not

A local DNS service for development environments. Developers and coding agents
can run separate copies of a project and reach each service by name. Assigning
host ports to every copy means choosing unused ports, passing those numbers into
configuration, and keeping track of which environment owns them. DNS names avoid
that bookkeeping and work for HTTP, Postgres, MySQL, Redis, and other protocols.

Each name resolves to a container's own IP, so every copy can use the same port:
`redis.project1.docker:6379` and `redis.project2.docker:6379` connect to different
addresses. The connection goes directly to the selected container.

This provides names pointing directly to container addresses. It does not proxy
traffic, terminate TLS, or route HTTP requests by hostname or path. Use an HTTP
proxy if several HTTP servers should sit behind one domain and port.

Hostname routing and port routing are different. Traefik can distinguish HTTP
requests by their host header, or TLS connections by SNI. Its plain TCP support
does not provide hostname routing for arbitrary protocols: the DNS name used by
the client is not carried in a TCP connection. Routing by port on one proxy IP
still requires distinct listening ports for different backends. It does not
replace DNS pointing to separate container IPs for this use case.

DNS does not solve CORS. Different hostnames or ports are still different browser
origins. Configure CORS in your application, or use an HTTP proxy to serve the
frontend and API through the [same origin](https://developer.mozilla.org/en-US/docs/Web/Security/Same-origin_policy).

Containers still need a shared network or a working route to reach each other.
This does not connect isolated networks or publish services to your LAN. It
assumes native, rootful Docker on Linux, rather than Docker Desktop's VM networking.

## Install

Requirements:

- Linux on x86_64 or ARM64 (`aarch64`).
- Rootful Docker managed by `docker.service`, using the local Unix socket.
- Active systemd-resolved, with systemd 258 or newer.
- Host DNS configured to use resolved through NSS or its resolver stub.
- Bash and sudo for the install/uninstall scripts.

### From a release

Download the archive matching `uname -m` and `SHA256SUMS` from this repository's
GitHub Releases page. No Rust installation is needed. For example, on x86_64:

```bash
sha256sum --ignore-missing --check SHA256SUMS
tar -xzf plop-dns-0.2.0-linux-x86_64.tar.gz
cd plop-dns-0.2.0-linux-x86_64
./install.sh
```

On ARM64, use `plop-dns-0.2.0-linux-aarch64.tar.gz`. Keep the extracted directory
to run uninstall later. To upgrade, extract the new release and run its installer.
Installation leaves inactive Docker asleep; DNS starts when Docker starts normally.

### From source

```bash
cargo build --release --locked
./scripts/install.sh
```

### DNS inside containers

Omarchy already directs container DNS to host resolved at `172.17.0.1:53`.
No Docker DNS changes or Compose `dns:` settings are needed there.

On other distributions, check whether containers can resolve a running service
with `nslookup web.myproject.docker`. Seeing `127.0.0.11` in a container's
`/etc/resolv.conf` means it uses Docker's embedded resolver; that resolver's
upstream still needs to reach host resolved.

If forwarding is not configured, this matches Omarchy's setup for a rootful
Docker bridge at `172.17.0.1`:

```ini
# /etc/systemd/resolved.conf.d/20-docker-dns.conf
[Resolve]
DNSStubListenerExtra=172.17.0.1
```

Merge this property into `/etc/docker/daemon.json`, preserving existing settings:

```json
{ "dns": ["172.17.0.1"] }
```

Use your actual host bridge address if it differs. Once the bridge exists, apply
the settings:

```bash
sudo systemctl restart docker.service
sudo systemctl restart systemd-resolved.service
```

Restarting Docker may interrupt running containers. Allow UDP/TCP port 53 from
containers to that host address in your firewall. These host-wide settings are
separate from plop-dns; its installer and uninstaller do not change them.

### Uninstall

From the extracted release directory:

```bash
./uninstall.sh
```

For a source checkout, use `./scripts/uninstall.sh`.

This stops and disables the service, removes its binary and DNS delegation, then
reloads resolved. Docker containers, networks, and existing host DNS settings stay
in place.

## Usage

Start your Compose project normally. A service named `web` in project `myproject`
gets these names automatically:

```text
web.myproject.docker      all running web replicas
web-1.myproject.docker    replica 1
web-2.myproject.docker    replica 2, when present
```

Connect using the service's actual port, without a Compose `ports:` mapping. For
a web server listening on port 3000:

```bash
curl http://web.myproject.docker:3000
```

The names work from the host and from containers whose DNS forwards through
resolved, provided the selected container address is reachable.

### Service labels

Add labels under an existing Compose service:

```yaml
services:
  web:
    labels:
      dns: special
      dns.global: shared
      dns.default: "true"
      dns.network: workspace-services
```

| Label         | Effect                                                                                                                |
| ------------- | --------------------------------------------------------------------------------------------------------------------- |
| `dns`         | Adds an alias alongside the automatic names. `special` and `special.docker` both register `special.<project>.docker`. |
| `dns.global`  | Adds a global alias: `shared` and `shared.docker` both register `shared.docker`, shared across projects.              |
| `dns.default` | With `"true"`, also registers the service at `<project>.docker`. Quote the boolean value.                             |
| `dns.network` | Uses addresses from this attached Docker network for all of the container's records. Overrides network priorities.    |

Aliases already ending in the current project name, such as
`dns: "special.${COMPOSE_PROJECT_NAME}"` or
`dns: "special.${COMPOSE_PROJECT_NAME}.docker"`, keep that project suffix once.
Both `dns` and `dns.global` can be set on the same service. These are container
labels under `labels:`, separate from Compose’s service-level `dns:` setting.
To keep an existing unscoped alias, move it from `dns` to `dns.global`.

Both labels also accept comma-separated lists, mixing exact and wildcard aliases:

```yaml
labels:
  dns: "x,*.x"
  dns.global: "y,*.y,*.*.y"
```

In project `myproject`, `dns: "x,*.x"` registers `x.myproject.docker` and
matches names such as `a.x.myproject.docker`. The global list registers
`y.docker` and matches `a.y.docker` and `a.b.y.docker`. Spaces around items
are trimmed, empty items are ignored, and duplicate aliases do not duplicate
addresses. Invalid items are logged and skipped while valid items still work.
Project and `.docker` suffixes are handled separately for each item.

For `dns.network`, use the actual Docker network name; Compose may prefix it with
the project name. If that network is not attached, the container is skipped and
the reason is logged.

Multiple containers can register the same alias or project name. Their addresses
are combined, duplicates removed, and the first returned address rotates between
queries.

### Wildcard aliases

Both alias labels accept `*` as a complete DNS label. Each `*` matches exactly
one nonempty label; quote wildcard values in YAML:

```yaml
labels:
  dns: "*.x"
  dns.global: "*.*.y"
```

In project `myproject`, these match `a.x.myproject.docker` and
`a.b.y.docker`, respectively. `dns: "*.*.y"` would instead match
`a.b.y.myproject.docker`; `dns.global: "*.x"` would match `a.x.docker`.
The optional `.docker` suffix and existing project suffix work as for ordinary
aliases. Partial wildcards such as `app*` are invalid.

Exact registered names take precedence, including names with no addresses.
Otherwise, the matching pattern with the fewest wildcards wins; ties prefer
literal labels closest to `.docker`. Containers registering the same pattern
combine their addresses and round robin. Answers use the requested hostname.
A wildcard does not match its base name or extra levels: `*.x` matches `a.x`
but not `x` or `a.b.x` (before the project and zone suffixes).

### Network labels

Set priorities on the top-level Compose networks:

```yaml
networks:
  shared:
    labels:
      dns.priority: "1"
  background:
    labels:
      dns.priority: "999"
```

For containers without `dns.network`, the attached network with the lowest
priority wins. The default is 500: use 1 to prefer a network or 999 to deprioritize
it. Ties prefer the container's primary network, then alphabetical network name.
Priorities are unsigned integers; invalid values are logged and treated as 500.
For an external network, set the label where that network is created.

### Check DNS

```bash
resolvectl query web.myproject.docker
dig @127.0.0.1 -p 5354 web.myproject.docker
systemctl status plop-dns.service
journalctl -u plop-dns.service -n 50 --no-pager
```

In Alpine, install [bind-tools](https://pkgs.alpinelinux.org/contents?arch=x86_64&branch=edge&name=bind-tools&repo=main)
as root, then test the container's normal DNS path:

```sh
apk add --no-cache bind-tools
nslookup web.myproject.docker
```

Replace the example name with your running service and project.

## Guides

- [Shared services across projects](docs/use-cases/01-shared-services.md): run one
  MySQL, Redis, or other shared service for several projects.
- [Agentic Compose environments](docs/use-cases/02-agentic-compose-environments.md):
  give each agent and worktree its own reachable environment.
- [Network binding](docs/examples/01-network-binding.md): select the network used
  for a container's DNS records.
- [Network priority](docs/examples/02-network-priority.md): prefer shared networks
  and deprioritize others.
- [Project-scoped aliases](docs/examples/03-project-scoped-aliases.md): give repeated
  applications distinct custom names using `${COMPOSE_PROJECT_NAME}`.
- [Troubleshooting](docs/troubleshooting.md): check aliases, DNS forwarding,
  networks, and connectivity.

## Architecture and details

The daemon runs as a native systemd service. Resolved routes `.docker` queries to
it through a DNS delegation file:

```text
host      -> resolved -> .docker -> plop-dns
container -> Docker DNS -> resolved -> .docker -> plop-dns
```

The listener is fixed at `127.0.0.1:5354`, and the zone is `.docker`. The daemon
answers only for this zone; resolved handles ordinary DNS through existing
upstreams. There is no upstream resolver or forwarding in plop-dns.

`scripts/install.sh` embeds the systemd configuration and installs it at:

```text
/etc/systemd/system/plop-dns.service
/etc/systemd/dns-delegate.d/30-docker-domains.dns-delegate
```

The delegation sets `DefaultRoute=no`, so unrelated lookups do not go to this
service. Install and uninstall reload resolved to add or remove that delegation.

### Discovery and Docker lifecycle

Discovery takes a snapshot of running containers and network labels, then waits
for container start/exit/removal and network attachment events. Immediately
available events are drained before one refresh; events still arriving can
trigger the next refresh. Idle Docker receives no periodic requests.

After a failed event stream, discovery refreshes the snapshot and reconnects
with a new client after two seconds. If refresh also fails or Docker is inactive,
records are cleared. Errors are reported from the third consecutive failure;
a successfully handled event resets the count. Events since the initial snapshot
started are replayed to cover startup changes.

The unit is enabled under `docker.service`, without a dependency that starts
Docker. The binary checks systemd's service state before connecting to Docker's
activation socket. DNS queries read memory and never contact Docker. Systemd
stops DNS before Docker; use the unit because a manually run binary's state check
is not atomic against concurrent Docker shutdown.

### DNS behavior

UDP/TCP and IPv4/IPv6 records are supported. All records have TTL 0. Unknown local
names return NXDOMAIN, unavailable discovery returns SERVFAIL, and queries outside
`.docker` are refused. A project name without `dns.default` exists with no
addresses and returns an empty successful answer. One-off Compose run containers
and invalid hostname labels are skipped.

Round robin rotates the first address independently for each name and record
type. It does not check application health or control which address a client
ultimately uses.

## Alternatives

### Docker's embedded DNS

[Docker's embedded DNS](https://docs.docker.com/engine/network/#dns-services)
(`127.0.0.11`) resolves service names inside a container's attached Docker
networks. It lets containers on a shared network find a service as `nginx`, but
does not give the host a directory of all Compose projects.

plop-dns adds names that distinguish those projects and work from the host:

```text
nginx.project1.docker:80
nginx.project2.docker:80
```

Docker's embedded DNS still handles names within shared networks. plop-dns
adds project-qualified names and custom aliases through resolved; it does not
replace Docker's network-local discovery. Neither bypasses network isolation:
resolving a name does not make the container's IP reachable.

### Other options

- **[CoreDNS with a Docker discovery plugin](https://coredns.io/explugins/docker/):** provides Docker records through a
  general DNS server. Useful when you need more DNS features or configurable
  zones; Docker discovery plugins are external to CoreDNS.
- **[Traefik](https://doc.traefik.io/traefik/v3.3/routing/routers/), Caddy, or nginx:** use a reverse proxy for HTTP hostname/path routing
  or TLS SNI routing through a shared IP and port. Plain TCP port routing does not
  distinguish arbitrary services by hostname on that same IP and port. A proxy
  can be used alongside plop-dns, including to serve one HTTP origin for CORS.
- **Published Docker ports:** use them when services need to be reachable through
  host ports, including from other machines. Separate copies need distinct host
  ports or another routing arrangement.

Licensed under the [MIT license](LICENSE).
