# Bind DNS records to a network

A container attached to several networks has an address on each. A client needs
the address on a network it can reach. Use the service label `dns.network` to
choose which network supplies that container's DNS records.

Save this as `compose.yaml`:

```yaml
services:
  web:
    image: nginx:alpine
    networks:
      - frontend
      - backend
    labels:
      dns: binding-web.docker
      dns.default: "true"
      dns.network: binding-frontend

  client:
    image: alpine:3.23
    command: ["sleep", "infinity"]
    networks:
      - frontend

networks:
  frontend:
    name: binding-frontend
    driver: bridge
  backend:
    name: binding-backend
    driver: bridge
```

Start it as project `binding`:

```bash
docker compose -p binding up -d
```

All of `web`'s names use its `binding-frontend` address:

```text
web.binding.docker
web-1.binding.docker
binding-web.docker
binding.docker
```

The client shares that network, so it can reach nginx through these names. From
the host, nginx is available without a published port:

```bash
curl http://web.binding.docker/
```

To check DNS inside the client:

```bash
docker compose -p binding exec client apk add --no-cache bind-tools
docker compose -p binding exec client nslookup web.binding.docker
```

This assumes [container DNS forwards through host resolved](../../README.md#dns-inside-containers).

## Use the actual network name

The label takes the Docker network name, not the Compose network key. Here,
`frontend` is the key and `binding-frontend` is the actual name because of `name:`.
Without an explicit name, Compose normally prefixes the key with the project
name, such as `binding_frontend`.
[Compose network names](https://docs.docker.com/reference/compose-file/networks/#name)

The example's fixed network names are intended for this single demonstration.
For parallel environments, keep project-scoped names and set `dns.network` to
each environment's actual network name.

`dns.network` overrides [network priorities](02-network-priority.md). If the named
network is not attached, the container is skipped and the reason is logged. The
daemon does not silently use another network.

This selects the addresses advertised in DNS; it does not change the DNS server's
listening address or attach the container to a network. The choice applies to
all automatic names and aliases, and to both IPv4 and IPv6.
