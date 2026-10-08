# Project-scoped custom aliases

The `dns` label automatically scopes a custom alias to its Compose project.
The same alias can be used in multiple environments without combining their
addresses. Use `dns.global` when a name should be shared across projects.

Save this as `compose.yaml`:

```yaml
services:
  web:
    image: nginx:alpine
    labels:
      dns: app
```

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

`dns: app.docker` also registers `app.<project>.docker`. Existing explicitly
qualified labels, `dns: "app.${COMPOSE_PROJECT_NAME}"` and
`dns: "app.${COMPOSE_PROJECT_NAME}.docker"`, still work: the current project
suffix is added only if it is missing. Use project names without dots or
underscores, such as `fix-42`.

For a shared name, add `dns.global: app` (or `dns.global: app.docker`). This
registers `app.docker`; containers in different projects using that global alias
combine their addresses and round robin. Both labels may be set together.

Replicas within one project still share its alias and round robin. To address a
specific replica, use its automatic name, such as `web-1.fix-42.docker`.

## Wildcard aliases

For names one level below an alias, use `dns: "*.x"`. In project `fix-42`,
this resolves `tenant.x.fix-42.docker`. For two levels, use `dns: "*.*.y"`,
which resolves `tenant.region.y.fix-42.docker`.

The global equivalents are `dns.global: "*.x"` for `tenant.x.docker` and
`dns.global: "*.*.y"` for `tenant.region.y.docker`. Always quote values
containing `*` in YAML. Each wildcard matches exactly one label; exact names
and more specific patterns take precedence. See
[wildcard aliases](../../README.md#wildcard-aliases) for matching rules.

To register both a base name and its wildcard, use a comma-separated list:

```yaml
labels:
  dns: "x,*.x"
  dns.global: "y,*.y,*.*.y"
```

In project `fix-42`, the scoped list serves `x.fix-42.docker` and
`tenant.x.fix-42.docker`. The global list serves `y.docker`, `tenant.y.docker`,
and `tenant.region.y.docker`. Each item follows the same suffix and matching
rules as a single alias.
