# Troubleshooting

Replace `web.myproject.docker` below with your actual service and Compose project.
The examples assume Docker is already running. Check service state with systemd;
Docker CLI commands can activate an otherwise sleeping Docker daemon.

## Why is DNS switching between services?

When several containers register the same name, docker-dns combines their
addresses and rotates the first answer between queries. This can happen when:

- Several containers have the same `dns` alias, even in different projects.
- Several services in the same project have `dns.default: "true"`.
- A Compose service has several running replicas.

Inspect the labels of running services in the project:

```bash
docker ps --filter label=com.docker.compose.project=myproject \
  --format '{{.Names}} dns={{.Label "dns"}} default={{.Label "dns.default"}}'
```

Use distinct aliases if the services should be separate. Mark only the intended
entry service as default, or use `<service>.<project>.docker`. To target a single
replica, use a name such as `web-1.myproject.docker`.

Round robin is not a health check and does not guarantee which address a client
will use. A client may reorder answers or retain an existing connection.

## Why does it not work?

Check DNS separately from connectivity. A successful lookup can still return an
address your client cannot reach.

### 1. Check the services

```bash
systemctl status docker.service docker-dns.service systemd-resolved.service
journalctl -u docker-dns.service -n 50 --no-pager
```

docker-dns intentionally stays stopped when Docker is inactive. It does not start
Docker in response to a DNS request. Installation requires systemd 258 or newer,
rootful Docker, and an active systemd-resolved service.

### 2. Query the daemon directly, then resolved

```bash
dig @127.0.0.1 -p 5354 web.myproject.docker
resolvectl query web.myproject.docker
```

If the direct query works but resolved does not, check the installed delegation:

```bash
cat /etc/systemd/dns-delegate.d/30-docker-domains.dns-delegate
```

It should contain:

```ini
[Delegate]
DNS=127.0.0.1:5354
Domains=~docker
DefaultRoute=no
```

Run the installer again to restore this file and reload resolved. If `resolvectl`
works but ordinary applications do not, make sure host DNS uses resolved through
NSS or its resolver stub.

### 3. Check DNS inside the client container

On Alpine, run these commands as root inside the client container:

```sh
apk add --no-cache bind-tools
cat /etc/resolv.conf
nslookup web.myproject.docker
```

`127.0.0.11` is Docker's embedded resolver. Its upstream must use host resolved
for `.docker` names to work. Omarchy configures this through `172.17.0.1`; on other
systems follow the [container DNS setup](../README.md#dns-inside-containers).

For Omarchy's bridge address, test resolved directly from the container:

```sh
nslookup web.myproject.docker 172.17.0.1
```

If that works but the normal lookup does not, check Docker's configured DNS
upstream and any service-level `dns:` overrides. If it fails while host lookups
work, check resolved's `DNSStubListenerExtra`, the host bridge address, and
firewall access to UDP/TCP port 53. Do not set a container's DNS to host-local
`127.0.0.1`; that is the container's own loopback address.

### 4. Check bridge networking and the selected address

This setup targets native Linux bridge networks. Inspect the networks attached
to the target and client, then the network driver:

```bash
docker inspect --format '{{json .NetworkSettings.Networks}}' target-container
docker inspect --format '{{json .NetworkSettings.Networks}}' client-container
docker network inspect --format '{{.Driver}}' actual-network-name
```

The client needs a shared bridge network or a working route to the advertised
address. Two different bridge networks are still isolated. If the target has
several networks, [select one with dns.network](examples/01-network-binding.md)
or [set network priorities](examples/02-network-priority.md).

## Why do I get NXDOMAIN or an empty answer?

NXDOMAIN means the name is not registered. Check the service and project names,
that the container is running, and its effective labels. Plain `docker run`
containers without Compose labels and one-off `docker compose run` containers
are not registered.

Automatic project and service names must be valid DNS labels: letters, digits,
and hyphens, with no leading/trailing hyphen. Underscores, dots, or oversized
labels cause the container to be skipped. The journal reports invalid labels.

`dns.network` must name an attached Docker network, including any Compose project
prefix. A missing selected network skips the container's records; it does not
fall back to another network.

An empty successful answer for `<project>.docker` is normal when no service is
marked `dns.default`. An empty AAAA answer is also normal for an IPv4-only target.

## Why do I get SERVFAIL?

Discovery is unavailable: Docker may be stopped, the initial snapshot may still
be loading, or Docker API access may have failed. Check the service journal and
Docker's systemd state. If discovery cannot recover a fresh snapshot, records are
cleared rather than continuing to advertise an unavailable snapshot. Errors are reported
from the third consecutive failure.

## DNS resolves, but the connection fails

Use the container's listening port, not a previous published host port. A service
listening on 3000 is reached as `http://web.myproject.docker:3000`, unless an HTTP
proxy provides another entry point.

Check that the application listens on `0.0.0.0` or its container interface, not
only `127.0.0.1` inside the container. Also check network access, firewall rules,
and application readiness. A running container is registered before its
application is necessarily ready to accept connections.

## Why is CORS still failing?

DNS does not change browser origins. Different hostnames or ports are different
origins even if both addresses belong to the same project. Configure CORS in the
application, or place the frontend and API behind one HTTP proxy and origin.

## Why do all agents reach the same environment?

Give every worktree a distinct Compose project name. Reusing `-p myproject`
means Compose operates on the same project instead of creating separate copies.
Use automatic project-qualified names or include the project name in custom
aliases: `dns: "app.${COMPOSE_PROJECT_NAME}.docker"`. See
[project-scoped aliases](examples/03-project-scoped-aliases.md) and
[agentic Compose environments](use-cases/02-agentic-compose-environments.md).

## Why do lookups outside .docker fail when querying the daemon directly?

docker-dns serves only `.docker` and refuses other zones. Query resolved or the
container's normal DNS resolver for external names; those resolvers handle the
existing upstream path. There is no fallback DNS server in docker-dns.
