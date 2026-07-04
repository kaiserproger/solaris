# M92 Autoscale Control Plane

Quality label: `draft`.

M92 added a bounded, local runtime-control primitive. It exposes three
profiles (`low_end`, `balanced`, `high_end`), deterministic limits, and
observable decisions for chunk/view throughput. As of the M100 stabilization
slices, enabled autoscale is wired into the live chunk-stream hot path for
chunk send-rate and for a prepare-dispatch cap derived from load/generate
limits. It does not implement or claim transparent shared-world horizontal
sharding, health/drain readiness, slow-client shedding, or profile-matrix soak.

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

## Hysteresis And Bounds

Pressure must persist for `scale_down_after_ticks` before limits are
reduced. Healthy observations must persist for `scale_up_after_ticks`
before throughput is restored. Scaling is clamped by the selected profile
and explicit config overrides, so low-end degradation reduces
view/throughput rather than changing correctness semantics.

`RuntimeControlPlane::request_drain` clamps to minimum limits and keeps
subsequent observations in an observable drain state.

## Known Draft Gaps

- Live runtime control currently affects chunk send-rate and the prepare
  dispatch budget derived from load/generate limits. It does not yet provide
  runtime view-distance replanning or separately metered load-vs-generate
  queues.
- No health endpoint or in-game admin command consumes the snapshot yet.
- Slow-client load shedding and safe config reload are not implemented.
- No low-end/balanced/high-end performance soak was run in this slice.
- High-end throughput claims remain blocked on M91/M93 performance data.
