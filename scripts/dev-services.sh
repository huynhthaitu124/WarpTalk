#!/usr/bin/env bash
# Start local Postgres (trust auth, port 5432) and Redis for integration tests.
# Idempotent. Uses a private cluster under /tmp/pg; stops the distro cluster if it holds 5432.
set -euo pipefail
B=/usr/lib/postgresql/16/bin
if ! psql -h localhost -U postgres -d warptalk -c 'select 1' >/dev/null 2>&1; then
  pg_ctlcluster 16 main stop 2>/dev/null || true
  if [ ! -d /tmp/pg/data ]; then
    mkdir -p /tmp/pg && chown postgres /tmp/pg
    su postgres -c "$B/initdb -D /tmp/pg/data -A trust -U postgres" >/dev/null
  fi
  rm -f /tmp/pg/data/postmaster.pid
  su postgres -c "$B/pg_ctl -D /tmp/pg/data -o '-p 5432 -k /tmp' -l /tmp/pg/log start" >/dev/null
  sleep 2
  psql -h localhost -U postgres -tc "select 1 from pg_database where datname='warptalk'" | grep -q 1 \
    || psql -h localhost -U postgres -c 'create database warptalk' >/dev/null
fi
redis-cli ping >/dev/null 2>&1 || redis-server --port 6379 --daemonize yes >/dev/null
echo "postgres+redis ready"
