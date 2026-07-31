# Solaris code-review remediation plan

Date: 2026-07-31

Quality label: `stabilization`

Source bundle: `tmp/SOLARIS_CODE_REVIEW_BUNDLE/`

Source snapshot SHA-256: `1770a1bca05f435e9c242a7d2276e9b9842db7c95b1c5fde6ef28c7d57983185`

## Scope and evidence boundary

The Pro review is a static analysis of a Repomix subset. It did not include every ignored/generated/data file, root workspace manifests, dependency lock state, CI policy, runtime configuration, live clients, profilers, fault injection, or cargo validation. Its findings are therefore a remediation input, not executable evidence by themselves.

Every finding must pass current-tree triage before code changes:

1. reproduce or confirm the stated control/data flow in the current checkout;
2. check whether later local commits already fixed or narrowed it;
3. add a focused failing regression when practical;
4. repair the shared primitive rather than one call site;
5. run the affected package gates and the validation tier required by `AGENTS.md`;
6. record remaining deployment, oracle, client, performance, concurrency, and recovery gaps honestly.

The bundle contains 42 findings: 11 High, 26 Medium, and 5 Low. The review marks 27 as release blockers. Public exposure and write-format migration remain stabilization-only until the confirmed blocker set is closed or explicitly accepted by the owner.

## Main conclusions

The strongest parts of Solaris remain its explicit ownership direction, bounded queues in several subsystems, lack of `unsafe`, deterministic evidence culture, and focused runtime tests. The review identifies a repeated cross-cutting weakness: important invariants are enforced locally instead of at shared boundaries.

The remediation program therefore prioritizes common primitives:

- exact packet decode before liveness credit;
- absolute state-machine deadlines and bounded unauthenticated admission;
- parser allocation/byte/node budgets;
- validated resource paths and canonical identifiers;
- storage coordinate/size invariants before mutation;
- authority-side movement validation;
- bounded actor/plugin delivery and typed fail-stop supervision.

## Ordered work waves

### Wave A — Canonical data and parser safety

Close small, reusable invariant primitives first because they reduce the blast radius of later storage and loader work.

- `SOL-023`: canonical `Identifier` serde; strict legacy object migration.
- `SOL-024`: separate validated resource path from logical identifier; prevent traversal and symlink escape.
- `SOL-016`: journal semantic image/frame/file budgets before allocation.
- `SOL-035`: bounded count-prefixed vector decoding based on remaining wire bytes.
- `SOL-009`: aggregate NBT byte/node/string/entry budgets and writer rollback.
- `SOL-021`: bounded playerdata/structure decompression with exact consumption.
- `SOL-032`: iterative bounded data-sidecar traversal with explicit symlink policy.

Exit condition: malformed tiny inputs cannot trigger disproportionate allocation, panic, or path escape in the covered primitives.

### Wave B — World persistence integrity

- `SOL-013`: bounded Anvil region/chunk decompression and aggregate memory.
- `SOL-014`: reject invalid coordinates, duplicate slots, and chunks over 255 sectors without modifying output.
- `SOL-015`: validate reserved sectors, extents, overlap, and codec checksums.
- `SOL-020`: enforce `requested/cache key == decoded chunk.pos == serialized expected key`.
- `SOL-017`: acquire an OS-level exclusive lease for a writable world root.
- `SOL-027`: enforce resident/dirty byte budgets and backpressure when save is unhealthy.
- `SOL-040`: centralize cross-platform atomic replace and durability policy.

Exit condition: corrupt/imported files fail closed or quarantine, failed writes leave prior files intact, and two writable processes cannot share a world root.

### Wave C — Network liveness and admission

- `SOL-001`: only exact, recognized decoded packets count as inbound liveness.
- `SOL-004`: exact Configuration packet bodies, including empty ACKs.
- `SOL-003`: absolute handshake/Configuration/Loader deadlines and bounded unauthenticated permits.
- `SOL-028`: weighted per-session packet/byte rate budgets.
- `SOL-029`: deadlines and shutdown/owner-health selection for simulation queue and replies.
- `SOL-007`: bounded Mojang response reads and correct upstream-status classification.
- `SOL-008`: canonical compression framing, strict stream completion, and CPU budgeting.
- `SOL-030`: canonical login-name validation and identity collision policy.

Exit condition: slow or malformed clients cannot retain slots indefinitely, monopolize permits, or block the connection loop behind an unhealthy owner.

### Wave D — Runtime ownership and fail-stop behavior

- `SOL-002`: close-safe async extension boundary; no lost close wakeups.
- `SOL-012`: bounded committed-event bridge with explicit required/best-effort policy.
- `SOL-031`: typed owner fatal state and coordinated fail-stop instead of panic propagation.
- `SOL-041`: classify poisoned locks; reset benign telemetry only, fail-stop authoritative state.
- `SOL-037`: remove stale admission tombstones with RAII cancellation.
- `SOL-039`: strict production plugin deployment mode and expected plugin set.
- `SOL-038`: aggregate plugin wall-clock/fairness budget.

Exit condition: owner/plugin failure completes pending work with typed errors, stops new mutation, and enters a controlled drain/shutdown path without indefinite waiters.

### Wave E — Authority and gameplay correctness

- `SOL-019`: authority-side displacement budget, swept collision, loaded destination, and teleport exceptions.
- `SOL-025`: preserve valid partial horizontal travel and report collision separately.

Exit condition: a client cannot choose arbitrary movement, while legitimate lag/knockback/swim/vehicle/teleport paths remain explicitly modeled.

### Wave F — Loader, data correctness, and architecture debt

- `SOL-005`: bind Loader artifact digest to immutable/opened bytes.
- `SOL-006`: validate ACK state IDs and carrier family against the negotiated registry snapshot.
- `SOL-018`: either implement dependency/load-order graph validation or remove unsupported contract fields.
- `SOL-022`: validate resource-heavy configuration maxima and checked derived budgets.
- `SOL-010`, `SOL-011`, `SOL-026`, `SOL-033`, `SOL-034`, `SOL-036`: harden public codec/physics/recipe/tag/item-stack contracts.
- `SOL-042`: extract typed dispatch/deadline/transaction middleware after characterization tests; do not mechanically split files without ownership boundaries.

## Initial status

| Finding | State | Evidence / next action |
| --- | --- | --- |
| `SOL-023` | complete | Derived serde was replaced by canonical string serialization. The legacy `{full,colon}` object is accepted only when both fields exactly match the value re-parsed through `Identifier::parse`; impossible indices, invalid characters, non-canonical bare paths, and extra fields fail closed. `mc-data`, `mc-protocol`, `mc-world`, `mc-script`, and `mc-worldgen` package tests plus workspace Clippy pass. |
| `SOL-024` | complete | Added a shared `ResourcePath` boundary that rejects empty/dot/absolute/platform-ambiguous segments. Registry Network-NBT, entity-loot references, configured worldgen features, and ore features now read from an already-opened descriptor whose OS identity is compared with the post-open canonical target under the trusted root. The independent reviewer initially blocked the canonicalize-then-reopen TOCTOU; that path was removed and a deterministic symlink-swap regression now fails closed through `same-file` identity checks. Primitive plus real loader traversal/symlink tests pass; the only remaining direct `join(identifier.path())` occurrence is a trusted test fixture writer. No second reviewer was run, per `AGENTS.md`. |
| `SOL-016` | complete | World-journal recovery validates image count against a 512-image semantic ceiling and remaining payload before allocation, caps each image NBT at 16 MiB, frame payloads at 64 MiB, the journal file at 256 MiB, and pending decisions at 65,536. Encoding preflights aggregate append/replacement size and uses fallible reservations. Recovery reads an exact bounded file length and repairs an incomplete tail through the same open handle. The writer owns an exclusive cross-process `fd-lock` lease, so size/header mutation is serialized; a reserved append that cannot complete enters fail-stop and wakes later waiters. The single reviewer initially returned `BLOCKED` for waiter deadlock, file-growth races, `MAX+1` recovery allocation, pathname reopen during repair, and aggregate buffer growth. All findings received direct regressions; no second reviewer was run per `AGENTS.md`. Focused journal tests pass 29/29 and full `mc-net` passes 1,863 tests plus 3 additional tests, with 5 ignored. Workspace Clippy, formatter, code-health, and diff-check pass. The final `cargo test --workspace` attempt was externally SIGTERM after 58 seconds without a reported failed test and is not counted as a complete workspace gate. |
| `SOL-035` | complete | Added shared `read_bounded_count`, `read_bounded_vec`, and bounded initial-capacity primitives for VarInt-counted collections. Counts are checked for negativity, semantic maxima, and `count × conservative minimum wire size` feasibility before the first collection allocation; multiplication and `needed` arithmetic saturate safely, and initial capacity is capped at 1,024 while valid larger collections grow after successful element decoding. Login profile properties, Play dimension/player/entity/command/chunk/light/recipe collections, Configuration registries/tags/known packs, merchant offers, and entity-attribute collections were migrated. The residual allocation inventory contains no eager `with_capacity`/`reserve` from wire counts outside the helper. Production malformed-frame regressions cover Login properties, dimension names, one-million chunk block entities, and one-million tag entries; helper regressions cover negative/over-max/tiny-body/overflow/bounded-capacity/valid-large cases. `mc-protocol` passes 314 tests, `mc-net` passes 1,863 plus 3 additional tests with 5 ignored, all `mc-server` suites pass, and the full `mc-test-harness` suite passes. Workspace Clippy, formatter, code-health, and diff-check pass. The final `cargo test --workspace` attempt was externally SIGTERM after 61 seconds without a reported failed test and is not counted as a complete workspace gate. The single bounded reviewer attempt was externally terminated before producing a new verdict; its output file remained the stale prior `SOL-016` review, so no second reviewer was started per `AGENTS.md`. |
| `SOL-009` | next | Confirm current NBT aggregate byte/node/string/entry budgets and writer rollback behavior before implementation. |
| Other findings | queued for current-tree triage | Confirm against current code before implementation; static bundle severity is not automatically treated as current runtime truth. |

## Validation policy

Each checkpoint is committed separately. The ZIP, extracted review bundle, `repomix-output.xml`, local worlds, `.analysis/`, and other owner artifacts are never staged. A finding is not marked complete from helper-only tests: the touched boundary must have focused malformed/sad-path coverage, and storage/network/runtime findings require the relevant concurrency, persistence, or liveness evidence.
