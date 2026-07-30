# Deterministic worldgen mosaic evidence

## Scope

This checkpoint renders the current production `tellus_like` surface sampler
without generating or loading chunks. The renderer uses the same terrain,
biome, and vegetation decisions as `TerrainGenerator`; the images are
diagnostic views, not a second worldgen authority.

The checked seed set is exactly `[712816]`. Each image covers the half-open
block extent `x = [-1024, 1024)`, `z = [-1024, 1024)`, centered at `(0, 0)`.
One pixel covers an `8x8` block cell, so every `256x256` PNG represents exactly
`2048x2048` blocks.

Reproduce the checked artifacts from the repository root:

```sh
cargo run -p mc-worldgen --bin worldgen_mosaic -- \
  --seed 712816 \
  --center-x 0 \
  --center-z 0 \
  --extent 2048 \
  --blocks-per-pixel 8 \
  --output-dir docs/evidence/worldgen-mosaics/seed-712816
```

The command always samples
`WorldgenMode::TellusLike(TellusWorldgenSettings::default())` and writes
`height.png`, `biome.png`, and `vegetation.png`.

## Fixed palettes

- Height uses sea level `63` as its boundary. At or below sea level,
  `depth = clamp(63 - y, 0, 127)` maps to
  `RGB(18, 80 - depth / 3, 150 - depth / 2)`. Above sea level,
  `altitude = clamp(y - 63, 0, 192)` maps to
  `RGB(46 + altitude, 96 + altitude / 2, 38 + altitude / 3)`. Channel
  arithmetic saturates to `u8`.
- Biomes use the first matching fixed family: deep ocean `#142D78`, ocean
  `#235AAA`, river `#377DCD`, beach/shore `#E2CF82`, badlands `#B8562D`,
  desert `#E0C460`, jungle `#1C7630`, swamp `#435B36`, snow/frozen/grove
  `#CDE0E6`, taiga `#46745C`, mountain/peak/stony/windswept `#7D7E78`,
  forest `#2C7E37`, savanna `#A4AD44`, and default `#67AB4A`.
- Vegetation reports the production biome's regional density. Unsupported
  biomes are `#34312B`. For supported biomes,
  `intensity = round((clamp(density, -1, 1) + 1) / 2 * 255)` and the color is
  `RGB(24 + intensity / 8, 48 + intensity * 3 / 4,
  27 + intensity / 4)`, again with saturating channel arithmetic.

## Rendered artifacts

| View | Artifact | SHA-256 |
| --- | --- | --- |
| Surface height | [`worldgen-mosaics/seed-712816/height.png`](worldgen-mosaics/seed-712816/height.png) | `4f143aa1037dfab8788e10b0ae372078f6b1aa80e701c9e8627bd8657955674d` |
| Biome family | [`worldgen-mosaics/seed-712816/biome.png`](worldgen-mosaics/seed-712816/biome.png) | `603d2f2f6368237548cabf868b0f7f37b18581c36c5bdb908c08c162eb447308` |
| Vegetation density | [`worldgen-mosaics/seed-712816/vegetation.png`](worldgen-mosaics/seed-712816/vegetation.png) | `2f35196886556fd7cf92cb90f5f60efa1e27035c15c86a2d091eb3cbf6171279` |

`file` identifies all three artifacts as non-interlaced 8-bit RGB PNGs at
`256x256`. A second complete CLI render was compared byte-for-byte with all
three checked artifacts and matched. The retained command logs are
`.analysis/codex-logs/worldgen-mosaic-{render,repeat}.log`; the repeat output
directory is intentionally ignored.

The mosaics visibly distinguish the large water bodies, land relief, biome
families, and supported vegetation-density regions. They also expose a narrow,
long east-west river corridor near the southern part of the rendered extent.
That observation is navigation context for the owner playtest, not a quality
disposition from this renderer checkpoint.

## Validation

- `cargo test -p mc-worldgen`: `121` passed, `2` documented ignores.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test --workspace`: PASS.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo fmt --all -- --check` and scoped diff/link checks: PASS.
- Independent read-only review: PASS with no findings; it rechecked the
  authority boundary, decoded dimensions, recorded checksums, and a complete
  byte-identical rerender.

Manual/client gate: not run. The public plan requires the owner-observed
seed-`712816` playtest next.

Benchmark: not applicable. This checkpoint adds diagnostic sampling and
rendering without changing generated-column decisions; the mapped release-host
throughput comparison remains open.

## Evidence boundary

The artifacts provide deterministic, reviewable height, biome, and vegetation
coverage for the public-alpha worldgen checkpoint. They do not establish
subjective terrain quality, ordinary client traversal, restart persistence, or
the release-host 225-chunk throughput comparison. Those gates remain separate.
