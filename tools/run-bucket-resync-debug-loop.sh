#!/usr/bin/env bash
# Deterministic feature-debug loop for bucket/block-resync behavior.
#
# The automated path runs one exact test at each fast validation layer. The
# optional real-client path uses one fixed manifest/scenario, a fresh world and
# a fixed seed so the same feature is exercised rather than whichever scenario
# happened to run most recently.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:---automated}"

MANIFEST="$REPO_ROOT/docs/real-client-regression/manifests/m94-regression-pack.json"
SCENARIO="m94-02b-rejected-block-resync"
SERVER_SEED="0"

cd "$REPO_ROOT"

run_unit() {
  cargo test -p mc-net --lib \
    play::bucket_interactions::tests::committed_bucket_response_orders_block_ack_before_inventory_update \
    -- --exact --nocapture
}

run_tcp_harness() {
  cargo test -p mc-test-harness --test block_edit \
    rejected_occupied_bucket_use_item_on_resyncs_blocks_and_held_slot_before_ack \
    -- --exact --ignored --nocapture
}

run_restart() {
  cargo test -p mc-test-harness --test block_edit \
    water_bucket_scheduled_spread_survives_save_restart_without_duplicate_tick \
    -- --exact --ignored --nocapture
}

run_real_client_check() {
  SOLARIS_REAL_CLIENT_MANIFEST="$MANIFEST" \
  SOLARIS_REAL_CLIENT_AGENT_SCENARIO="$SCENARIO" \
  SOLARIS_REAL_CLIENT_FRESH_WORLD=1 \
  SOLARIS_REAL_CLIENT_SERVER_SEED="$SERVER_SEED" \
    bash "$REPO_ROOT/tools/run-real-client-regression.sh" --check
}

run_real_client() {
  SOLARIS_REAL_CLIENT_MANIFEST="$MANIFEST" \
  SOLARIS_REAL_CLIENT_AGENT_SCENARIO="$SCENARIO" \
  SOLARIS_REAL_CLIENT_FRESH_WORLD=1 \
  SOLARIS_REAL_CLIENT_SERVER_SEED="$SERVER_SEED" \
    bash "$REPO_ROOT/tools/run-real-client-regression.sh" --run
}

case "$MODE" in
  --check)
    run_real_client_check
    ;;
  --automated)
    run_unit
    run_tcp_harness
    run_restart
    run_real_client_check
    ;;
  --real-client)
    run_real_client
    ;;
  --all)
    run_unit
    run_tcp_harness
    run_restart
    run_real_client
    ;;
  -h|--help)
    cat <<'EOF'
Usage: bash tools/run-bucket-resync-debug-loop.sh [--check|--automated|--real-client|--all]

  --check        Validate the exact declared real-client scenario and adapter.
  --automated    Run unit, TCP/harness, restart, then real-client preflight. Default.
  --real-client  Run only the exact graphical real-client scenario.
  --all          Run automated layers followed by the graphical real-client scenario.
EOF
    ;;
  *)
    printf 'error: unknown mode: %s\n' "$MODE" >&2
    exit 2
    ;;
esac
