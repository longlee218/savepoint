#!/usr/bin/env bash
# Boots the isolated provider fleet, runs every savepoint-state test against
# real Postgres / MySQL / MariaDB / Redis / Valkey / Redis Stack / Memcached /
# BullMQ, then tears the fleet down (pass --keep to leave it running).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/docker/providers.compose.yml" -p savepoint-providers)
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

cleanup() {
  if [[ $KEEP -eq 0 ]]; then
    "${COMPOSE[@]}" down -v >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

"${COMPOSE[@]}" up -d --wait

if [[ ! -d "$ROOT/crates/savepoint-state/tests/bullmq/node_modules" ]]; then
  (cd "$ROOT/crates/savepoint-state/tests/bullmq" && npm install --silent --no-audit --no-fund)
fi

# shellcheck source=provider-env.sh
source "$ROOT/scripts/provider-env.sh"
cd "$ROOT"
cargo test -p savepoint-state -- --test-threads=1 --nocapture
