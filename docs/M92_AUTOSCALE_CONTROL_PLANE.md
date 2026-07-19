# M92 Autoscale Control Plane

Quality label: `draft`.

M92 added a bounded, local runtime-control primitive. It exposes three
profiles (`low_end`, `balanced`, `high_end`), deterministic limits, and
observable decisions for chunk/view throughput and simulation work. As of the
M100 stabilization slices, enabled autoscale is wired into the live chunk-stream
hot path for chunk send-rate, runtime view-distance limits, separately metered
prepare-dispatch caps for classified load/generate work, bounded ECS
pathfinding/physics work, and bounded random and scheduled tick budgets. Every
100 simulation ticks, the existing runtime p95 window rebalances those budgets.
A saturated scheduled queue keeps its quota while random ticks yield first. It
does not implement or claim transparent
shared-world horizontal sharding, a production load-balancer readiness/drain
contract, broad slow-client shedding, or profile-matrix soak.

## Operator Surface

`mc-server --check --config example.toml` now renders an
`effective_autoscale` block. It reports whether autoscale is enabled, the
runtime mode, selected profile, normalized policy bounds, and the initial
limits the controller starts from when enabled.

Config shape:

```toml
[autoscale]
enabled = false
profile = "balanced"
min_view_distance = 6
max_view_distance = 10
target_tick_ms = 50
target_first_chunk_ms = 1500
scale_down_after_ticks = 3
scale_up_after_ticks = 10
```

All override fields are optional, and unknown `[autoscale]` fields are rejected.
Invalid or zero bounds normalize to at least one unit of work and a minimum
view distance of two.

The old `[simulation].random_tick_chunk_budget` and
`[simulation].scheduled_fluid_tick_budget` settings were removed and are now
rejected as unknown fields. With autoscale enabled, the selected profile and
runtime p95 observations own those budgets. With autoscale disabled, fixed
internal defaults are used. `random_tick_speed` remains configurable because it
is a gameplay rule, not a worker allocation knob.

## Decision Inputs

`mc_net::RuntimeControlPlane::observe` accepts a snapshot with:

- `tick_ms`
- `queued_chunks` and `queue_capacity`
- `active_workers` and `worker_capacity`
- `memory_used_mb` and `memory_limit_mb`
- `first_chunk_ms`

Pressure is classified as one of tick time, chunk queue, worker
saturation, memory, or first-chunk SLA. The returned
`AutoscaleDecision` includes action, pressure source, bounded limits, and
a reason string suitable for logs/status output.

`RuntimeControlPlane::observe_work` accepts tick, entity-goal, entity-physics,
entity-dispatch, random-tick, block-tick, and fluid-tick signals plus
scheduled-budget exhaustion. Percentile calculation stays off the tick path;
when the worker finishes an exact source window, it pushes that window directly
to the ticker and the controller applies it once. There is no boundary poll of
a potentially stale snapshot. Scheduled-budget exhaustion accumulates until a
window is accepted by the worker queue. Dispatch contributes to entity
pressure. The decision exposes the selected work class and bounded entity,
random, and scheduled budgets through runtime status JSON.

## Hysteresis And Bounds

Pressure must persist for `scale_down_after_ticks` before limits are
reduced. Healthy observations must persist for `scale_up_after_ticks`
before throughput is restored. Work-budget recovery adds half the remaining
distance to its ceiling per healthy decision instead of jumping directly to
the maximum. Scaling is clamped by the selected profile
and explicit config overrides, so low-end degradation reduces
view/throughput rather than changing correctness semantics.

`RuntimeControlPlane::request_drain` clamps to minimum limits and keeps
subsequent observations in an observable drain state.

## Known Draft Gaps

- Live runtime control has focused coverage for chunk send-rate, runtime
  view-distance replanning, separately metered load-vs-generate prepare
  dispatch, ECS pathfinding/physics work, and random/scheduled simulation
  budgets. Lighting, persistence, compression, and prewarm budgets are not yet
  owned by this controller. It also lacks profile-matrix soak and a production
  load-balancer readiness/drain contract.
- Authenticated `/status` exposes the runtime-control snapshot. Unauthenticated
  server-list `solaris.health` exposes only `ready` and `state`; readiness
  requires a serving lifecycle, supported authentication, an available world,
  and player capacity. Autoscale limits, pressure, counters, and reasons stay
  on the authenticated operator path. Startup/serve failure and external
  load-balancer behavior still need broader lifecycle coverage.
- Focused slow-client timeout and pre-timeout queue-pressure shedding paths
  exist, but broad slow-client recovery/rejoin policy and safe config reload are
  not implemented.
- No low-end/balanced/high-end performance soak was run in this slice.
- High-end throughput claims remain blocked on M91/M93 performance data.
