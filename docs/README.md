# Solaris documentation

Solaris 1.0.0-rc.1 "Pauline" is the release candidate for Solaris 1.0, for
Minecraft Java 26.1.2. Start with the [project README](../README.md) for
installation and a source quick start.

## Running and extending Solaris

- [Operating guide](OPERATING.md): configuration, content, console, backups and world conversion.
- [Migration from vanilla or Paper](MIGRATION.md): configuration mapping, offline import, preservation limits and rollback.
- [Settlements player guide](SETTLEMENTS_GUIDE.md) (English / Русский): `/settlement guide`, villager jobs and membership.
- [Server-driven dialogs](DIALOGS.md): `show_dialog` UI API and where clicks arrive.
- [Plugins](PLUGINS.md): Wasmtime package deployment and authoring.
- [Solaris Loader](SOLARIS_LOADER.md): optional client adapters and client bundles.
- [Project scope](PROJECT_SPEC.md) and [village generation](VILLAGE_GENERATION.md).
- [Release notes](releases/): versioned changes, not current configuration reference.

## Architecture

- [Architecture overview](ARCHITECTURE.md), [runtime map](architecture/runtime.md),
  [state ownership](architecture/state-ownership.md) and [mutation paths](architecture/mutations.md).
- [ADRs](decisions/README.md): binding decisions and explicit supersession status.
- [Generated crate graph](generated/crate-graph.md), [contract index](generated/contract-index.md)
  and [scenario index](generated/scenario-index.md): source-derived navigation.
- [Physics oracles](physics-oracles.md), [core profiling](performance/CORE_PROFILE_MATRIX.md)
  and [entity scale profiling](performance/ENTITY_SCALE_PROFILE.md),
  [v2 player capacity receipt](performance/PLAYER_CAPACITY.md),
  [weak-hardware / scaling / worldgen / VPS benchmarks](performance/BENCHMARKS.md).
- [Settlements owner decisions](settlements/OWNER_DECISIONS.md) and
  [warehouse binding](architecture/pilot-warehouse-binding.md): scoped domain contracts.

## Development and validation

- [Contributing](../CONTRIBUTING.md) and [agent working rules](../AGENTS.md).
- [Agent tooling and harness](AGENT_TOOLING.md): commands, profiles and prerequisites.
- [Definition of done](DEFINITION_OF_DONE.md) and [break-it QA protocol](QA_AGENT_PROTOCOL.md): breaker role, attack matrix, gate hygiene.
- [Quirk and farm parity gate](QUIRK_PARITY.md): vanilla-oracle proof that player-relied quirks and farm rates are preserved.
- [Vanilla compatibility matrix](COMPATIBILITY.md): 26.1.2 mechanics coverage per area.
- [Playable scenarios](playable/README.md) and [real-client regression manifests](real-client-regression/README.md).
- [Validation ledger](VALIDATION_LEDGER.md): retained input to `coverage_audit`, not a claim of current acceptance.
- [Optional core audit runbook](agents/PRO_CORE_AUDIT_RUNBOOK.md).

JSON files under `playable/` and `real-client-regression/manifests/`, generated
indexes, the validation ledger and referenced worldgen mosaic images are retained
because tooling consumes them. Historical process logs, old plans and audit dumps
are no longer published as current docs; archived versions remain in Git history.
Local agent state lives in ignored `.analysis/`, not in this documentation tree.
