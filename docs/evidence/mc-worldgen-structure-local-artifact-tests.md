# `mc-worldgen` structure local-artifact test classification

Scope: Phase 1 inventory of `mc-worldgen::structures` tests that require
ignored local Mojang data.

Three ordinary tests previously returned successfully when either the local
26.1.2 blocks report or the plains fountain NBT was absent. The same Cargo
result could therefore mean either “the structure assertions passed” or
“nothing ran.”

The tests are now explicit opt-in gates. Ordinary Cargo runs report them as
ignored, and an explicit ignored-test invocation fails immediately when either
declared prerequisite is missing.

## Inventory

| Test | Boundary | Owner and exact close condition |
| --- | --- | --- |
| `structures::tests::loads_real_plains_fountain_template_when_present` | Loads the real fountain NBT through the report-derived registry and checks its size, block count, and complete state resolution. | `mc-worldgen::structures` with the `mc-data` block-report loader. Close when the selected gate passes against the locally extracted target-version report and fountain template. |
| `structures::tests::loads_real_plains_village_prototype_when_present` | Builds the current one-template plains-village prototype and checks its combined bounds, block and villager-marker counts, and grid placement constants. | `mc-worldgen::structures`. Close when the selected gate passes against both declared 26.1.2 sidecars. This does not close broader village parity. |
| `structures::tests::plains_village_plan_selects_only_declared_building_parts_when_present` | Requests only the fountain prototype part and checks the exact source-template dimensions and a non-empty real block population. | `mc-worldgen::structures`. Close when the selected gate passes against both declared 26.1.2 sidecars. |

Total: three explicit local-artifact gates.

Always-executable synthetic structure tests remain the ordinary correctness
authority for template combination, offsets, placement and serialization. The
opt-in gates extend that coverage to local Mojang artifacts; they do not replace
it or prove complete vanilla village generation.

## Current disposition

The focused validation host has both declared 26.1.2 sidecars. All three
selected gates passed against them. The ordinary crate suite reports the gates
as ignored: its unit target passed 105 tests and reported five ignores total,
including the two separately classified performance probes. Its binary,
integration, and doc-test targets also passed.

The prerequisite checks are separate `is_file` assertions with their exact
paths. A deliberate invocation without either file fails before parsing instead
of returning success.

`benchmark: not applicable`: this checkpoint changes test classification only,
not a runtime path, and makes no performance claim.

## Reproduction

Run the always-executable suite:

```sh
cargo test -p mc-worldgen
```

List the complete ignored inventory:

```sh
cargo test -p mc-worldgen -- --list --ignored
```

After running `tools/extract-vanilla-data.sh`, select only these opt-in gates:

```sh
cargo test -p mc-worldgen \
  structures::tests::loads_real_plains_fountain_template_when_present \
  -- --ignored --exact
cargo test -p mc-worldgen \
  structures::tests::loads_real_plains_village_prototype_when_present \
  -- --ignored --exact
cargo test -p mc-worldgen \
  structures::tests::plains_village_plan_selects_only_declared_building_parts_when_present \
  -- --ignored --exact
```

Do not use an undifferentiated `cargo test -- --ignored` invocation: the same
crate also owns two independently classified performance probes.
