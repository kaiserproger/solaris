#!/usr/bin/env bash
set -u -o pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
RUNNER="$ROOT/tools/run-m79-oracle-suite.sh"
FIXTURES="$ROOT/.analysis/oracle-suite/runner-checks"

fail() {
  printf 'check-m79-oracle-runner: %s\n' "$1" >&2
  exit 1
}

write_manifest() {
  local root="$1"
  local id="${2:-spawn-smoke}"
  local kind="${3:-cargo-ignored-test}"
  mkdir -p "$root/tools/m79-oracle-scenarios"
  {
    printf 'id=%s\n' "$id"
    printf 'rows=P1,Q1\n'
    printf 'kind=%s\n' "$kind"
    printf 'captures=.analysis/oracle-suite/%s/\n' "$id"
    printf 'requires=.analysis/server.jar\n'
    printf 'consumes=.analysis/server.jar\n'
    printf 'consumes=data/vanilla/reports\n'
    printf 'consumes=.analysis/test-world\n'
    printf 'success_marker=M79_ORACLE_COMPARISON_OK %s\n' "$id"
    printf 'degrades_when_missing=data/vanilla/reports\n'
    printf 'degrades_when_missing=.analysis/test-world\n'
  } > "$root/tools/m79-oracle-scenarios/$id.manifest"
}

run_case() {
  local root="$1"
  local run_arg="${2:-}"
  local output="$root/stdout.txt"
  local status
  if M79_ORACLE_ROOT="$root" \
    M79_ORACLE_MANIFEST_DIR="$root/tools/m79-oracle-scenarios" \
    M79_ORACLE_JAVA="$root/bin/java" \
    M79_ORACLE_JAVAP="$root/bin/javap" \
    "$RUNNER" $run_arg > "$output" 2>&1; then
    status=0
  else
    status=$?
  fi
  printf '%s\n' "$status"
}

make_available_artifacts() {
  local root="$1"
  mkdir -p "$root/.analysis/test-world" "$root/data/vanilla/reports" "$root/bin"
  : > "$root/.analysis/server.jar"
  printf '#!/usr/bin/env sh\nexit 0\n' > "$root/bin/java"
  printf '#!/usr/bin/env sh\nexit 0\n' > "$root/bin/javap"
  chmod +x "$root/bin/java" "$root/bin/javap"
}

rm -rf "$FIXTURES"
mkdir -p "$FIXTURES"

missing_artifacts="$FIXTURES/missing-artifacts"
write_manifest "$missing_artifacts"
status="$(run_case "$missing_artifacts")"
[ "$status" -eq 20 ] || fail "missing artifacts inspect exited $status, want 20"
grep -q 'Overall: `blocked`' "$missing_artifacts/.analysis/oracle-suite/m79-report.md" || fail 'missing artifacts report is not blocked'
grep -q 'missing .analysis/server.jar' "$missing_artifacts/.analysis/oracle-suite/m79-report.md" || fail 'missing server.jar gap absent'

inspect_only="$FIXTURES/inspect-only"
write_manifest "$inspect_only"
make_available_artifacts "$inspect_only"
status="$(run_case "$inspect_only")"
[ "$status" -eq 10 ] || fail "inspect-only exited $status, want 10"
grep -q 'Overall: `degraded`' "$inspect_only/.analysis/oracle-suite/m79-report.md" || fail 'inspect-only report is not degraded'
grep -q 'not run; pass --run to consume oracle artifacts' "$inspect_only/.analysis/oracle-suite/m79-report.md" || fail 'inspect-only not-run gap absent'

missing_java="$FIXTURES/missing-java"
write_manifest "$missing_java"
make_available_artifacts "$missing_java"
output="$missing_java/stdout.txt"
if M79_ORACLE_ROOT="$missing_java" \
  M79_ORACLE_MANIFEST_DIR="$missing_java/tools/m79-oracle-scenarios" \
  M79_ORACLE_JAVA="$missing_java/no-java" \
  M79_ORACLE_JAVAP="$missing_java/no-javap" \
  "$RUNNER" > "$output" 2>&1; then
  status=0
else
  status=$?
fi
[ "$status" -eq 20 ] || fail "missing Java/javap exited $status, want 20"
grep -q 'missing executable java' "$missing_java/.analysis/oracle-suite/m79-report.md" || fail 'missing java gap absent'
grep -q 'missing executable javap' "$missing_java/.analysis/oracle-suite/m79-report.md" || fail 'missing javap gap absent'

run_pass="$FIXTURES/run-pass"
write_manifest "$run_pass" pass runner-self-check
make_available_artifacts "$run_pass"
status="$(run_case "$run_pass" --run)"
[ "$status" -eq 0 ] || fail "--run pass exited $status, want 0"
grep -q 'Overall: `full`' "$run_pass/.analysis/oracle-suite/m79-report.md" || fail '--run pass report is not full'
grep -q 'marker M79_ORACLE_COMPARISON_OK pass' "$run_pass/.analysis/oracle-suite/m79-report.md" || fail '--run pass marker evidence absent'

run_skip="$FIXTURES/run-skip"
write_manifest "$run_skip" skip runner-self-check
make_available_artifacts "$run_skip"
status="$(run_case "$run_skip" --run)"
[ "$status" -eq 20 ] || fail "--run skip exited $status, want 20"
grep -q 'scenario skipped instead of comparing oracle output' "$run_skip/.analysis/oracle-suite/m79-report.md" || fail '--run skip gap absent'

run_no_marker="$FIXTURES/run-no-marker"
write_manifest "$run_no_marker" no-marker runner-self-check
make_available_artifacts "$run_no_marker"
status="$(run_case "$run_no_marker" --run)"
[ "$status" -eq 20 ] || fail "--run no-marker exited $status, want 20"
grep -q 'missing positive oracle comparison marker' "$run_no_marker/.analysis/oracle-suite/m79-report.md" || fail '--run no-marker gap absent'

printf 'check-m79-oracle-runner: ok\n'
