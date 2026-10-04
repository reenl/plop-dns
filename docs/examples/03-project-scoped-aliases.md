# Project-scoped custom aliases

When the same application runs in several Compose projects, include the project
name in its custom alias. Otherwise, a shared alias such as `app.docker` combines
the addresses from every environment and round robins between them.

Save this as `compose.yaml`:

```yaml
services:
  web:
    image: nginx:alpine
    labels:
      dns: "app.${COMPOSE_PROJECT_NAME}.docker"
```

Compose interpolates `${COMPOSE_PROJECT_NAME}` using the selected project name,
including one supplied with `-p`. See Docker's
[project-name documentation](https://docs.docker.com/reference/compose-file/version-and-name/).

Start two copies of the same application:

```bash
docker compose -p fix-42 up -d
docker compose -p fix-73 up -d
```

Each environment gets its own alias, alongside the automatic service names:

| Project  | Custom alias        | Automatic service name |
| -------- | ------------------- | ---------------------- |
| `fix-42` | `app.fix-42.docker` | `web.fix-42.docker`    |
| `fix-73` | `app.fix-73.docker` | `web.fix-73.docker`    |

```bash
curl http://app.fix-42.docker/
curl http://app.fix-73.docker/
```

Both containers listen on port 80 without publishing a host port. The default
Compose networks use the bridge driver; follow the [installation guide](../../README.md#install)
to configure host and container DNS.

You can omit `.docker` from the label: `dns: "app.${COMPOSE_PROJECT_NAME}"`
registers the same alias because plop-dns appends the zone. Use project names
without dots or underscores, such as `fix-42`.

Replicas within one project still share its alias and round robin. To address a
specific replica, use its automatic name, such as `web-1.fix-42.docker`.
