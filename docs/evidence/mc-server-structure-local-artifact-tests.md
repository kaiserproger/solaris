# mc-server local structure artifact tests

## Inventory and disposition

| Test | Boundary | Current disposition |
| --- | --- | --- |
| `tests::settlement_profile_loads_the_extracted_prototype_when_present` | Loads the extracted plains fountain NBT through `structure_rules_for_startup` and checks that the template has substantial block content. | Explicitly ignored by ordinary test runs; passes when selected with the local sidecar present and fails closed when it is absent. |
| `tests::extracted_village_prototype_generates_deterministically_when_present` | Generates the same village area twice, compares every generated block, and checks a substantial difference from structure-free terrain. | Explicitly ignored by ordinary test runs; passes when selected with the local sidecar present and fails closed when it is absent. |

## Artifact ownership and close condition

The repository does not own or distribute Mojang's
`data/vanilla/data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt`.
The local test operator owns extracting that sidecar before selecting these gates.
The exact close condition for either ignored gate is that this path exists as a
regular file and the selected test passes; absence is an assertion failure, not
a successful skip.

## Reproduction

From the repository root with the local sidecar installed:

```sh
cargo test -p mc-server --bin mc-server tests::settlement_profile_loads_the_extracted_prototype_when_present -- --ignored --exact
cargo test -p mc-server --bin mc-server tests::extracted_village_prototype_generates_deterministically_when_present -- --ignored --exact
```

Run `cargo test -p mc-server` to confirm both gates remain visible as ignored in
the ordinary package test result. On the validation tree the binary unit target
reported `54 passed; 0 failed; 2 ignored`; the existing process-level ignored
integration gates remain classified separately.

## Limits

These tests prove only the local extracted plains fountain template load and its
deterministic Solaris generation boundary. They do not prove extraction
correctness, redistribution safety, other village templates, broader structure
placement parity, or vanilla-client gameplay parity.

`benchmark: not applicable`: this checkpoint changes test classification only,
not a runtime path, and makes no performance claim.
