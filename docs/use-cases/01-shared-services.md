# Shared services across projects

Run one MySQL, Redis, or other shared service and connect several Compose projects
to it. Each project uses the same DNS name and normal service port, without
publishing ports or copying container IPs into application configuration.

The projects need a shared bridge network. DNS provides the name; the network
provides connectivity.

## Start the shared services once

Save this as `shared-services.compose.yaml`:

```yaml
services:
  mysql:
    image: mysql:8.4
    environment:
      MYSQL_ROOT_PASSWORD: dev-root-password
      MYSQL_DATABASE: app
      MYSQL_USER: app
      MYSQL_PASSWORD: dev-app-password
    volumes:
      - mysql-data:/var/lib/mysql
    networks:
      - shared

  redis:
    image: redis:7-alpine
    networks:
      - shared

networks:
  shared:
    name: workspace-services
    driver: bridge
    labels:
      dns.priority: "1"

volumes:
  mysql-data:
```

The credentials here are development examples. Start the project:

```bash
docker compose -f shared-services.compose.yaml -p shared-services up -d
```

plop-dns registers these names automatically:

```text
mysql.shared-services.docker:3306
redis.shared-services.docker:6379
```

Wait for MySQL to finish initialization before connecting. The MySQL volume
belongs to this shared project, so application projects can come and go without
recreating the database server.

## Connect application projects

In each application's Compose file, attach its service to the existing shared
network and use the shared service names. Adapt the environment variables to your
application:

```yaml
services:
  app:
    build: .
    environment:
      DB_HOST: mysql.shared-services.docker
      DB_PORT: "3306"
      DB_DATABASE: app
      DB_USERNAME: app
      DB_PASSWORD: dev-app-password
      REDIS_HOST: redis.shared-services.docker
      REDIS_PORT: "6379"
    networks:
      - default
      - shared

networks:
  default:
    driver: bridge
  shared:
    external: true
    name: workspace-services
```

Start each project with its own project name. Both connect to the same MySQL and
Redis instances. There are no host `ports:` mappings in either file.

`external: true` tells Compose to reuse the network created by the shared-services
project. Set its labels in that owning project, not in the external declaration.
[Compose network documentation](https://docs.docker.com/reference/compose-file/networks/#external)

## Decide whether data should be shared

The configuration above deliberately shares the `app` database and Redis data.
If projects need independent state, create a database and user per project on the
same MySQL server, and use separate Redis key prefixes. Do not run concurrent
schema migrations against the same database unless that is intentional.

Other services can follow the same arrangement: a single Postgres server with
separate databases, or an S3-compatible server with separate buckets. Use each
service's normal port and its Compose-qualified DNS name.

Both container DNS and network access must work. On Omarchy, Docker already
forwards DNS through host resolved; elsewhere follow the
[container DNS setup](../../README.md#dns-inside-containers). If a shared service
has several networks, [bind its DNS records to the shared network](../examples/01-network-binding.md).

Keep shared services running while application projects use them. Stopping them
removes their DNS records and disconnects their clients. Stop application projects
independently; avoid taking down the owning network while other projects use it.
