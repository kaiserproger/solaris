# Architecture Decision Index

Read this index, then only the ADR that owns the decision. Later ADRs take
priority where they explicitly narrow or replace an older design. Milestone
notes record progress; they do not silently change an ADR.

| ADR | Status | Scope |
| --- | --- | --- |
| [0001](0001-vanilla-data-as-runtime-input.md) | Accepted | Mojang data as local runtime/build input; vendor bytes stay out of git |
| [0002](0002-vanilla-protocol-metadata-as-reference.md) | Accepted | Protocol metadata, oracle provenance, and packet-layout proof |
| [0003](0003-runtime-world-lock-architecture.md) | Accepted for legacy paths; staged supersession by ADR 0004 | Runtime world-lock structure and lock-order constraints |
| [0004](0004-staged-single-writer-simulation.md) | Accepted, staged migration | Single-writer authority, commands, publication, and migration fences |
| [0005](0005-regional-simulation.md) | Accepted, staged migration | Regional ownership, phases, transfers, WAL, and cross-region rules |
| [0006](0006-mc-net-module-boundaries.md) | Accepted, staged migration | `mc-net` module ownership and extraction boundaries |
| [0007](0007-connection-liveness-under-outbound-pressure.md) | Accepted | Keepalive liveness and bounded entity movement publication under pressure |
| [0008](0008-overworld-density-router.md) | Accepted, staged extraction | Single worldgen shape authority and deterministic chunk pipeline |

## Routing

- Data loading or licensing boundary: 0001.
- Packet IDs, layouts, or vanilla protocol evidence: 0002.
- Lock scope without an authority change: 0003.
- Simulation authority, commit, or publication: 0004.
- Regional ownership, transfer, ECS cutover interaction, or WAL ordering: 0005.
- `play.rs`, `session.rs`, or `simulation.rs` modularization: 0006.
- Connection liveness, keepalive timeout, or client movement pressure: 0007.
- Worldgen topology, climate, rivers, caves, or feature-pipeline ownership: 0008.

Update the owning ADR in the same slice when authority, threading, waiting,
persistence ordering, or module policy changes. State non-goals and staged
scope explicitly.
