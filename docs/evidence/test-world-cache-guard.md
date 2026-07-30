# Test-world generator cache guard

Scope: Phase 1 environment-sensitive success-path inventory for
`tools/generate-test-world.sh`.

The generator previously exited successfully when
`.analysis/test-world/region/r.0.0.mca` merely existed. A zero-byte region or an
interrupted copy with no `level.dat` was therefore accepted as a complete
vanilla fixture even though the producer's successful output contains both
non-empty files.

The cache hit now requires a non-empty `region/r.0.0.mca` and a non-empty
`level.dat`. If either output exists without the complete pair, the script fails
with an explicit removal/regeneration instruction. It does not silently delete
a possibly operator-inspected partial artifact.

Owner: the local Anvil-oracle fixture workflow. Close the guard only when a
synthetic complete pair exits before Java/bundle validation, while a
region-only or zero-byte pair fails before those prerequisites. Regenerating
the actual Mojang fixture remains an explicit operator action.

## Current disposition

Synthetic temporary-directory checks cover the complete cache hit, a non-empty
region without `level.dat`, and a zero-byte pair. Shell syntax validation also
passes. No Mojang server was started and no local `.analysis/test-world`
artifact was changed.

`benchmark: not applicable`: this changes only local fixture validation and no
runtime or generation path.

## Reproduction

```sh
bash -n tools/generate-test-world.sh

fixture_root="$(mktemp -d)"
mkdir -p "$fixture_root/complete/region"
printf region > "$fixture_root/complete/region/r.0.0.mca"
printf level > "$fixture_root/complete/level.dat"
OUT_DIR="$fixture_root/complete" \
  tools/generate-test-world.sh "$fixture_root/missing.jar"

mkdir -p "$fixture_root/partial/region"
printf region > "$fixture_root/partial/region/r.0.0.mca"
! OUT_DIR="$fixture_root/partial" \
  tools/generate-test-world.sh "$fixture_root/missing.jar"

mkdir -p "$fixture_root/empty/region"
: > "$fixture_root/empty/region/r.0.0.mca"
: > "$fixture_root/empty/level.dat"
! OUT_DIR="$fixture_root/empty" \
  tools/generate-test-world.sh "$fixture_root/missing.jar"
```

Use a fresh temporary directory for each reproduction and remove it afterward.
