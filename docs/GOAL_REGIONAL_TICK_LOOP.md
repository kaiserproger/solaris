# `/goal` Loop: Regional Entity Tick Perf Cursor

Owner-authorized iterative loop for one cursor. Run in-session, repeat until
`done_when` or hard budget. This suspends two standing rules **for this
cursor only**: the AGENTS.md fresh-session-per-checkpoint rule, and the
earlier do-not-relaunch note in `docs/MEMORY.md`.

```yaml
goal_checkpoint:
  north_star_ref: solaris-v1
  route: architecture (measured optimization / regional ownership / ECS)
  outcome: >
    Close the fixed 50/60 ms gate on the living-world workload, or record
    the exact measured blocker.
  done_when:
    - Fixed profile (50 clients, 25 regions, 4000/region, living-world mix,
      200 warmup + 1200 measured ticks) reports whole-tick p95 <= 50 ms and
      p99 <= 60 ms with zero reliable drops and 50/50 sessions retained, OR
      a concrete measured blocker names the exact remaining bucket.
    - Full workload, phase order, and coordinator fallback preserved
      (no cadence/LOD/cohort/physics-skip cheats unless owner approves).
    - Focused tests + fmt + code-health green on the final tree; skipped
      gates recorded exactly.
  primary_context: docs/MEMORY.md  # "Profile in flight" cursor bullet
```

## Loop

1. Read the cursor bullet in `docs/MEMORY.md`. It names the log dir holding
   the latest fixed profile.
2. Own the CPU: at most one profile run at a time. If relaunching, kill any
   stale `load_scenarios` run first and use a fresh `OUT_DIR`
   (`.analysis/bench/living-world-100k-<slice>`).
3. One iteration, in order:
   a. Fixed profile: `SOLARIS_ENTITY_BENCH_OUT_DIR=<fresh dir>
      SOLARIS_ENTITY_BENCH_MODE=baseline tools/profile-living-world-scale.sh`.
      The latency assertion is expected to fail; the numbers are the product.
   b. Percentiles (measured ticks only) of `OWNER_GOAL_PHASE` sub-buckets
      `resolve_scan_us` / `resolve_pathing_us` / `resolve_overrides_us`
      plus whole-tick/phase p50/p95/p99 from the assertion line.
   c. Remove exactly ONE measured redundant materialization in the worst
      bucket. Smallest direct local diff; no behavior change.
   d. Validate: `cargo test -p mc-entity`, `cargo fmt --all -- --check`,
      `cargo run -p xtask -- code-health`.
   e. Matched comparison profile (step a again, new dir). Claim only what
      the two runs prove; whole-tick numbers decide.
   f. Update the cursor bullet: latest dirs, bucket percentiles, decision.
4. Repeat from step 2 until `done_when` or hard budget.

## Rules

- Same session throughout; do not snapshot/compact just to continue the loop.
- Relaunch is allowed and required for matched before/after comparison.
  Never run two profiles concurrently; never validate on a loaded CPU.
- Never rerun a green gate on an unchanged tree identity.
- No gameplay, protocol, persistence, or plugin-surface changes in this loop.
- At hard budget: close `partial` or `checkpoint-blocked`, keep the same
  outcome, leave a one-action cursor. Do not shrink the outcome to fit.
```

Iteration budget (owner default, overrides AGENTS.md checkpoint budget here):

```yaml
model_roundtrips_soft: 8
model_roundtrips_hard: 12
shell_batches: 6
subagents: 0        # no reviewer inside the loop; one review at final close
l2_validation_runs: 0  # L2 once at final close if outcome is complete
```
