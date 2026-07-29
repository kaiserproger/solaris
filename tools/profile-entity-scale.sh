#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT_DIR="${SOLARIS_ENTITY_BENCH_OUT_DIR:-.analysis/bench/entity-scale}"
if [[ "$OUT_DIR" != /* ]]; then
    OUT_DIR="$ROOT/$OUT_DIR"
fi
MODE="${SOLARIS_ENTITY_BENCH_MODE:-both}"
PERF_FREQUENCY="${SOLARIS_ENTITY_BENCH_PERF_FREQUENCY:-499}"
CPUSET="${SOLARIS_ENTITY_BENCH_CPUSET:-}"
mkdir -p "$OUT_DIR"

export SOLARIS_ENTITY_BENCH_CLIENTS="${SOLARIS_ENTITY_BENCH_CLIENTS:-60}"
export SOLARIS_ENTITY_BENCH_REGIONS="${SOLARIS_ENTITY_BENCH_REGIONS:-16}"
export SOLARIS_ENTITY_BENCH_ENTITIES_PER_REGION="${SOLARIS_ENTITY_BENCH_ENTITIES_PER_REGION:-2500}"
export SOLARIS_ENTITY_BENCH_WARMUP_TICKS="${SOLARIS_ENTITY_BENCH_WARMUP_TICKS:-200}"
export SOLARIS_ENTITY_BENCH_MEASURE_TICKS="${SOLARIS_ENTITY_BENCH_MEASURE_TICKS:-1200}"
export CARGO_PROFILE_RELEASE_DEBUG="${CARGO_PROFILE_RELEASE_DEBUG:-1}"
export CARGO_PROFILE_RELEASE_STRIP="${CARGO_PROFILE_RELEASE_STRIP:-false}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

if [[ ! -f data/vanilla/reports/blocks.json || ! -f data/vanilla/reports/registries.json ]]; then
    if [[ -f ../data/vanilla/reports/blocks.json && -f ../data/vanilla/reports/registries.json ]]; then
        mkdir -p data/vanilla
        [[ -e data/vanilla/reports ]] || ln -s ../../../data/vanilla/reports data/vanilla/reports
        [[ -e data/vanilla/data ]] || ln -s ../../../data/vanilla/data data/vanilla/data
    else
        echo "Missing local data/vanilla sidecars. Run tools/extract-vanilla-data.sh first." >&2
        exit 2
    fi
fi

TEST_ARGS=(
    test --release
    -p mc-test-harness
    --test load_scenarios
    entity_scale_40k_hostiles_60_clients_profile
    --
    --ignored
    --nocapture
    --test-threads=1
)

RUN_PREFIX=()
if [[ -n "$CPUSET" ]]; then
    RUN_PREFIX=(taskset -c "$CPUSET")
fi

{
    echo "timestamp_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "git_commit=$(git rev-parse HEAD)"
    echo "git_branch=$(git branch --show-current)"
    echo "rustc=$(rustc --version)"
    echo "cargo=$(cargo --version)"
    echo "kernel=$(uname -srmo)"
    echo "cpuset=${CPUSET:-unrestricted}"
    echo "clients=$SOLARIS_ENTITY_BENCH_CLIENTS"
    echo "regions=$SOLARIS_ENTITY_BENCH_REGIONS"
    echo "entities_per_region=$SOLARIS_ENTITY_BENCH_ENTITIES_PER_REGION"
    echo "warmup_ticks=$SOLARIS_ENTITY_BENCH_WARMUP_TICKS"
    echo "measure_ticks=$SOLARIS_ENTITY_BENCH_MEASURE_TICKS"
    command -v lscpu >/dev/null && lscpu
} > "$OUT_DIR/metadata.txt"

cargo test --release -p mc-test-harness --test load_scenarios --no-run >/dev/null

run_baseline() {
    local report="$OUT_DIR/baseline.json"
    local log="$OUT_DIR/baseline.log"
    local time_report="$OUT_DIR/baseline.time.txt"
    echo "Running baseline entity-scale benchmark..."
    SOLARIS_ENTITY_BENCH_REPORT="$report" \
        /usr/bin/time -v -o "$time_report" \
        "${RUN_PREFIX[@]}" cargo "${TEST_ARGS[@]}" 2>&1 | tee "$log"
}

perf_allowed() {
    command -v perf >/dev/null || return 1
    local paranoid
    paranoid="$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 99)"
    (( paranoid <= 1 ))
}

run_perf() {
    local report="$OUT_DIR/perf-run.json"
    local log="$OUT_DIR/perf-run.log"
    local time_report="$OUT_DIR/perf-run.time.txt"
    local data="$OUT_DIR/perf.data"
    local text_report="$OUT_DIR/perf-report.txt"
    echo "Running perf-recorded entity-scale benchmark..."
    SOLARIS_ENTITY_BENCH_REPORT="$report" \
        perf record --all-user -F "$PERF_FREQUENCY" --call-graph dwarf \
        -o "$data" -- \
        /usr/bin/time -v -o "$time_report" \
        "${RUN_PREFIX[@]}" cargo "${TEST_ARGS[@]}" 2>&1 | tee "$log"
    perf report --stdio --percent-limit 0.25 --sort comm,dso,symbol \
        -i "$data" > "$text_report"
    echo "perf data: $data"
    echo "perf report: $text_report"
}

case "$MODE" in
    baseline)
        run_baseline
        ;;
    perf)
        if ! perf_allowed; then
            echo "perf is unavailable: kernel.perf_event_paranoid=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo unknown)." >&2
            echo "Temporarily enable profiling with: sudo sysctl kernel.perf_event_paranoid=1" >&2
            exit 3
        fi
        run_perf
        ;;
    both)
        run_baseline
        if perf_allowed; then
            run_perf
        else
            echo "Skipping perf run: kernel.perf_event_paranoid=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo unknown)." >&2
            echo "To enable it temporarily: sudo sysctl kernel.perf_event_paranoid=1" >&2
        fi
        ;;
    *)
        echo "Unknown SOLARIS_ENTITY_BENCH_MODE=$MODE (expected baseline, perf, or both)." >&2
        exit 2
        ;;
esac

echo "Entity-scale artifacts: $OUT_DIR"
