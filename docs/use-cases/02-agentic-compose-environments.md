# Separate Compose environments for agents

Several agents can work on the same repository at once, each fixing a different
bug in its own Git worktree. Each worktree needs a running environment for tests,
browser checks, and manual review. Assigning a different host port to every copy
adds configuration that has little to do with the bug being fixed.

Use a different Compose project name for each worktree. plop-dns gives each
environment its own names while every service keeps the same internal port.

## Create the worktrees

From the original repository:

```bash
git worktree add -b fix-42 ../project-fix-42
git worktree add -b fix-73 ../project-fix-73
```

Each agent works in its assigned directory and branch. Suppose the project's
`compose.yaml` includes a web application listening on port 3000:

```yaml
services:
  web:
    build: .
    labels:
      dns.default: "true"
```

The application must listen on `0.0.0.0:3000` inside the container, rather than
only container-local `127.0.0.1`. No `ports:` mapping is needed.

## Start both environments

Run these commands from the original repository:

```bash
docker compose -f ../project-fix-42/compose.yaml -p fix-42 up -d --build
docker compose -f ../project-fix-73/compose.yaml -p fix-73 up -d --build
```

Build paths such as `build: .` are relative to the Compose file, so each environment
builds its own worktree's code. The environments are reachable independently:

```text
http://web.fix-42.docker:3000
http://web.fix-73.docker:3000
```

The `dns.default` label also provides these shorter names:

```text
http://fix-42.docker:3000
http://fix-73.docker:3000
```

Both applications use port 3000, on different container IPs. An agent or developer
on the host can open the correct environment without looking up a published port.

## Keep the environments independent

Use distinct project names, without dots or underscores: `fix-42`, `fix-73`, or
`agent-1` work with the daemon's DNS-label validation. Compose's default project
name comes from the directory, but explicit `-p` names make the association clear.

Let Compose scope containers, networks, and volumes by project. Avoid fixed
`container_name` values or globally named volumes for state that should be
independent. A global alias such as `dns: app.docker` on every worktree would
combine their addresses and round robin between environments. Use the automatic
project-qualified names, or scope a custom alias with
`dns: "app.${COMPOSE_PROJECT_NAME}.docker"`. See the
[project-scoped alias example](../examples/03-project-scoped-aliases.md) for a
Compose file that can run in multiple environments.

Only mark the intended entry service `dns.default`. Marking both the frontend and
API as default makes `<project>.docker` resolve to both services. To expose them
under one browser origin, put an HTTP proxy in each environment and mark that
proxy as default. DNS alone does not solve CORS.

For shared dependencies, use the [shared-services setup](01-shared-services.md).
Keep separate databases or Redis key prefixes when independent tests require
independent data. Test containers need a shared network with the services they
access; DNS resolution alone does not connect different project networks.

## Stop one environment

```bash
docker compose -f ../project-fix-42/compose.yaml -p fix-42 down
```

Its DNS records disappear, and `fix-73` keeps running. Named volumes are retained
unless you explicitly remove them.
