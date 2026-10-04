# Prefer a network with dns.priority

Set `dns.priority` on a Docker network to influence the address chosen for every
attached container without an explicit `dns.network` override. Lower values win;
the default is 500.

This lets a shared application network take precedence over per-project or
monitoring networks without adding a network-selection label to each service.

Save this as `compose.yaml`:

```yaml
services:
  web:
    image: nginx:alpine
    networks:
      - default
      - shared
      - monitoring

  worker:
    image: alpine:3.23
    command: ["sleep", "infinity"]
    networks:
      - default
      - shared

networks:
  default:
    driver: bridge
  shared:
    name: priority-shared
    driver: bridge
    labels:
      dns.priority: "1"
  monitoring:
    name: priority-monitoring
    driver: bridge
    labels:
      dns.priority: "999"
```

Start it as project `priority`:

```bash
docker compose -p priority up -d
```

| Network | Priority | Selection |
| --- | --- | --- |
| `priority-shared` | 1 | Preferred for both services. |
| `priority_default` | 500 | Used if no lower-priority-number network is attached. |
| `priority-monitoring` | 999 | Deprioritized, but still usable. |

`web.priority.docker` and `worker.priority.docker` use their respective addresses
on `priority-shared`. Network order in the service's YAML does not override the
priority.

Check nginx from the host or DNS from the worker:

```bash
curl http://web.priority.docker/
docker compose -p priority exec worker apk add --no-cache bind-tools
docker compose -p priority exec worker nslookup web.priority.docker
```

This assumes [container DNS forwards through host resolved](../../README.md#dns-inside-containers).

## Override one service

To select a different attached network for `web`, add a service label:

```yaml
services:
  web:
    labels:
      dns.network: priority-monitoring
```

That explicit choice wins even though `priority-monitoring` has priority 999.
It affects only `web`; `worker` still uses priority-based selection. The worker
does not share the monitoring network, so this override would make the advertised
web address unsuitable for that client.

## Defaults and ties

A missing priority means 500. If priorities tie, the container's primary network
wins; remaining ties use alphabetical network name. Invalid values are logged
and treated as 500. Use unsigned integers, quoted as label strings in Compose.

999 is a preference, not an exclusion. A container attached only to that network
still uses its address.

For a shared external network, set the label when creating it in the owning
Compose project. The consuming projects declare only `external: true` and its
name. Compose does not accept extra network attributes on external declarations.
[Compose external networks](https://docs.docker.com/reference/compose-file/networks/#external)

The example uses fixed names for its shared and monitoring networks. Use
project-scoped names for networks that should be independent between environments.
