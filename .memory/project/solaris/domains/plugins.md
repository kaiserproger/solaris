# Plugin Route

Read `docs/PLUGINS.md` for the exact current API and ADR 0006 for ownership.
Do not infer plugin contracts from examples or old milestone notes.

Ownership:

- `crates/mc-script/` owns manifest/config validation, Luau VM limits, immutable
  DTO conversion, handler execution, command admission, and host-local timers.
- `crates/mc-net/src/script/` owns bounded server adapters and registries.
- Focused play/session endpoint modules own mutations requiring a connected
  player or authoritative simulation turn.
- `examples/plugins/` are shipped consumers. Their real behavior is exercised
  through `crates/mc-test-harness/tests/plugin_*.rs`. The current audit is
  `docs/EXAMPLE_PLUGIN_AUDIT.md`: economy, claims, and roster are complete only
  for their stated bounded scopes; the colony example is a partial domain
  plugin; geological mines and settlement remain declarative selectors needing
  more open startup data schemas.
- Rust owns mechanics and authority, not plugin vocabulary. Colony identity,
  homes, roles, orders, limits, and persistence live in Luau. The `villagers`
  capability exposes only an opaque expiring binding and bounded
  `idle`/`follow_position` goals through the regional owner.

Stable contract rules:

- Privileged calls require declared capabilities. Plugin identity comes from
  the attached host, never a Luau-supplied id.
- Events describe committed facts. Rejected, preview, stale, no-op, or partial
  paths must not fabricate success events.
- Owner/result events are targeted and correlated; do not replace exact result
  events with polling, elapsed time, or `server.tick` as a generic fence.
- Mutations cross the existing session/simulation/storage authority. Luau never
  receives live Rust references, locks, packet state, or direct ECS access.

For a new API slice, prove Luau validation, capability rejection, authoritative
success and material rejection paths, targeted isolation, and one production
TCP/Luau path. Run all workspace gates once when the completed slice is ready to
commit.
