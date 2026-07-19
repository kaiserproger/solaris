#!/usr/bin/env bash
set -u -o pipefail

ROOT="${M79_ORACLE_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
MANIFEST_DIR="${M79_ORACLE_MANIFEST_DIR:-$ROOT/tools/m79-oracle-scenarios}"
REPORT_DIR="$ROOT/.analysis/oracle-suite"
REPORT="$REPORT_DIR/m79-report.md"
RUN=0

usage() {
  printf 'usage: %s [--run]\n' "$(basename "$0")"
  printf '  without --run: inspect manifests and report degraded/blocked readiness\n'
  printf '  with --run: run scenarios whose required local artifacts are present\n'
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --run)
      RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

manifest_value() {
  local key="$1"
  local file="$2"
  while IFS='=' read -r name value; do
    [ "$name" = "$key" ] && printf '%s\n' "$value"
  done < "$file"
}

manifest_values() {
  local key="$1"
  local file="$2"
  while IFS='=' read -r name value; do
    [ "$name" = "$key" ] && printf '%s\n' "$value"
  done < "$file"
}

artifact_is_consumed() {
  local artifact="$1"
  local file="$2"
  local consumed
  while IFS= read -r consumed; do
    [ "$consumed" = "$artifact" ] && return 0
  done < <(manifest_values consumes "$file")
  return 1
}

run_scenario() {
  local kind="$1"
  local id="$2"

  case "$kind:$id" in
    cargo-ignored-test:configuration-phase)
      (cd "$ROOT" && cargo test -p mc-test-harness --test parity_oracle vanilla_and_solaris_configuration_phase_can_be_diffed -- --ignored --exact --nocapture)
      ;;
    cargo-ignored-test:core-actions-seed-81)
      (cd "$ROOT" && cargo test -p mc-test-harness --test parity_oracle checked_manifest_vanilla_and_solaris_protocol_observations_can_be_diffed -- --ignored --exact --nocapture)
      ;;
    cargo-ignored-test:spawn-smoke)
      (cd "$ROOT" && cargo test -p mc-test-harness --test parity_oracle vanilla_and_solaris_spawn_smoke_can_be_diffed -- --ignored --exact --nocapture)
      ;;
    runner-self-check:pass)
      printf 'M79_ORACLE_COMPARISON_OK %s\n' "$id"
      ;;
    runner-self-check:skip)
      printf 'skipping vanilla-backed parity test: fake JavaTooOld availability\n'
      ;;
    runner-self-check:no-marker)
      printf 'test exited successfully without oracle comparison\n'
      ;;
    *)
      printf 'unsupported scenario kind/id: %s/%s\n' "$kind" "$id" >&2
      return 64
      ;;
  esac
}

run_log_is_skip() {
  local log="$1"
  grep -Eq 'skipping vanilla-backed parity test|requires Java 25\+|JavaTooOld|missing availability|0 passed; 0 failed; [0-9]+ ignored' "$log"
}

run_log_has_success_marker() {
  local log="$1"
  local marker="$2"
  grep -Fq "$marker" "$log"
}

preflight_status="full"
preflight_gaps=()
JAVA_BIN="${M79_ORACLE_JAVA:-$(command -v java 2>/dev/null || true)}"
JAVAP_BIN="${M79_ORACLE_JAVAP:-$(command -v javap 2>/dev/null || true)}"

if [ -z "$JAVA_BIN" ] || [ ! -x "$JAVA_BIN" ]; then
  preflight_status="blocked"
  preflight_gaps+=("missing executable java")
fi

if [ -z "$JAVAP_BIN" ] || [ ! -x "$JAVAP_BIN" ]; then
  preflight_status="blocked"
  preflight_gaps+=("missing executable javap")
fi

mkdir -p "$REPORT_DIR"

{
  printf '# M79 Vanilla Oracle Suite Report\n\n'
  printf 'mode=%s\n\n' "$([ "$RUN" -eq 1 ] && printf run || printf inspect)"
  printf '| Scenario | Rows | Status | Evidence | Gaps |\n'
  printf '|---|---|---|---|---|\n'
} > "$REPORT"

overall="full"
found=0

for manifest in "$MANIFEST_DIR"/*.manifest; do
  [ -e "$manifest" ] || continue
  found=1
  id="$(manifest_value id "$manifest")"
  rows="$(manifest_value rows "$manifest")"
  kind="$(manifest_value kind "$manifest")"
  capture_dir="$(manifest_value captures "$manifest")"
  success_marker="$(manifest_value success_marker "$manifest")"
  [ -n "$success_marker" ] || success_marker="M79_ORACLE_COMPARISON_OK"
  status="$preflight_status"
  gaps=("${preflight_gaps[@]}")
  evidence="manifest only"

  while IFS= read -r required; do
    [ -n "$required" ] || continue
    if [ ! -e "$ROOT/$required" ]; then
      status="blocked"
      gaps+=("missing $required")
    fi
  done < <(manifest_values requires "$manifest")

  while IFS= read -r degraded; do
    [ -n "$degraded" ] || continue
    if [ ! -e "$ROOT/$degraded" ]; then
      [ "$status" = "full" ] && status="degraded"
      gaps+=("missing $degraded")
    elif [ "$RUN" -eq 1 ] && ! artifact_is_consumed "$degraded" "$manifest"; then
      [ "$status" = "full" ] && status="degraded"
      gaps+=("$degraded present but not consumed by scenario")
    fi
  done < <(manifest_values degrades_when_missing "$manifest")

  if [ "$RUN" -eq 0 ]; then
    if [ "$status" = "full" ]; then
      status="degraded"
    fi
    gaps+=("not run; pass --run to consume oracle artifacts")
    evidence="manifest only; artifacts not consumed"
  fi

  if [ "$RUN" -eq 1 ] && [ "$status" != "blocked" ]; then
    mkdir -p "$ROOT/$capture_dir"
    log="$ROOT/$capture_dir/run.log"
    if run_scenario "$kind" "$id" > "$log" 2>&1; then
      if run_log_is_skip "$log"; then
        status="blocked"
        evidence="skipped; log $capture_dir/run.log"
        gaps+=("scenario skipped instead of comparing oracle output")
      elif run_log_has_success_marker "$log" "$success_marker"; then
        evidence="passed; log $capture_dir/run.log; marker $success_marker"
      else
        status="blocked"
        evidence="unproven; log $capture_dir/run.log"
        gaps+=("scenario output missing positive oracle comparison marker $success_marker")
      fi
    else
      status="blocked"
      evidence="failed; log $capture_dir/run.log"
      gaps+=("scenario command failed")
    fi
  elif [ "$RUN" -eq 1 ]; then
    evidence="not run; required oracle artifacts missing"
  fi

  if [ "$status" = "blocked" ]; then
    overall="blocked"
  elif [ "$status" = "degraded" ] && [ "$overall" = "full" ]; then
    overall="degraded"
  fi

  if [ "${#gaps[@]}" -eq 0 ]; then
    gap_text="none"
  else
    gap_text="${gaps[0]}"
    for ((i = 1; i < ${#gaps[@]}; i++)); do
      gap_text="$gap_text; ${gaps[$i]}"
    done
  fi

  printf '| `%s` | `%s` | `%s` | %s | %s |\n' \
    "$id" "$rows" "$status" "$evidence" "$gap_text" >> "$REPORT"
done

if [ "$found" -eq 0 ]; then
  overall="blocked"
  printf '| n/a | n/a | `blocked` | no manifests found | missing `%s` |\n' "$MANIFEST_DIR" >> "$REPORT"
fi

{
  printf '\nOverall: `%s`\n' "$overall"
  printf 'Report: `%s`\n' "${REPORT#$ROOT/}"
} >> "$REPORT"

printf 'M79 oracle suite: %s\n' "$overall"
printf 'report: %s\n' "$REPORT"

case "$overall" in
  full) exit 0 ;;
  degraded) exit 10 ;;
  blocked) exit 20 ;;
  *) exit 1 ;;
esac
