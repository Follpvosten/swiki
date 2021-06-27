set export

DOCKER := `which podman || which docker`
PG_CONTAINER := "postgres-swiki"
PG_PASSWORD := "abc123"

migrate: install-sqlx start-postgres
    cargo sqlx migrate run

install-sqlx:
    cargo install sqlx-cli --no-default-features --features postgres

start-postgres:
    #!/bin/sh
    if ! $DOCKER container logs $PG_CONTAINER >/dev/null 2>/dev/null; then
        podman run --rm -d \
            --name $PG_CONTAINER \
            -e POSTGRES_USER=swiki \
            -e POSTGRES_DB=swiki \
            -e POSTGRES_PASSWORD=$PG_PASSWORD \
            -p 5433:5432 \
            postgres:13-alpine
    fi

stop-postgres:
    #!/bin/sh
    podman container logs ${PG_CONTAINER} >/dev/null 2>/dev/null || exit 0
    podman container rm -f ${PG_CONTAINER} >/dev/null 2>/dev/null
