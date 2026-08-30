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
| Surface height | [`worldgen-mosaics/seed-712816/height.png`](worldgen-mosaics/seed-712816/height.png) | `861222d2a3fc634967e56af1f425feb998dce4687e03c31cf6e96ae47d089c4f` |
| Biome family | [`worldgen-mosaics/seed-712816/biome.png`](worldgen-mosaics/seed-712816/biome.png) | `a5c3a964d9872c1454cb3fd120f6838be9d6f6de116dc7720914a27b2ce1f8ae` |
| Vegetation density | [`worldgen-mosaics/seed-712816/vegetation.png`](worldgen-mosaics/seed-712816/vegetation.png) | `64c5235ec1229c01e2c4438686672e54ee4a0ddd36e5834771485dc25b01a17f` |

`file` identifies all three artifacts as non-interlaced 8-bit RGB PNGs at
`256x256`. A second complete CLI render was compared byte-for-byte with all
three checked artifacts and matched. The final repeat output directory is
`.analysis/worldgen-mosaics-repeat-final` and is intentionally ignored.

The mosaics visibly distinguish the large water bodies, land relief, biome
families, and supported vegetation-density regions. They also expose a narrow,
long east-west river corridor near the southern part of the rendered extent.
That observation is navigation context for the owner playtest, not a quality
disposition from this renderer checkpoint.

## Post-model multi-seed matrix

After the coherence model change, the same `2048x2048` production sampler was
run in parallel for seeds `0`, `34`, `712816`, and `832040`. These ignored
artifacts in `.analysis/worldgen-matrix-final/` are a visual/fingerprint matrix,
not a replacement for owner review:

| Seed | Height SHA-256 | Biome SHA-256 | Vegetation SHA-256 |
| --- | --- | --- | --- |
| `0` | `4ce51d20e15d89a3f7bc55afc3d27b4828ce1846a12ddeeeb1fba13782b4e308` | `6e0f030f2376f1d7292c3dc3a43c6ca2d709bfff5d270f2944cee7cf665dbd8` | `f3d9bc3e688d95d65aa4545d83d9a27750ab91c50d176e6eacaa3df844ad7076` |
| `34` | `250630efa594ec4ecd404e255cc0ad403fca2380f00af3cb383febcd073e12ec` | `1e248509886ff342271d3806e3312270860c64925465c4f6db965d92a61e77d8` | `c6cddc1c8372f124309f186d7cdbf5fed2fa3a0839720833a6f74e40a49215b7` |
| `712816` | `861222d2a3fc634967e56af1f425feb998dce4687e03c31cf6e96ae47d089c4f` | `a5c3a964d9872c1454cb3fd120f6838be9d6f6de116dc7720914a27b2ce1f8ae` | `64c5235ec1229c01e2c4438686672e54ee4a0ddd36e5834771485dc25b01a17f` |
| `832040` | `fcd142eb01bd92c4308ac349e2a1f32e984ab207b460bc2b80f2aaf3b959feba` | `4efdba8b949dbb4a8ecdd8719ecdd3e3bd23988573c4c7c8ba9538e0f388e25b` | `c6cddc1c8372f124309f186d7cdbf5fed2fa3a0839720833a6f74e40a49215b7` |


## Validation

- `cargo test -p mc-worldgen --lib`: `112` passed, `5` documented ignores.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo clippy -p mc-worldgen --all-targets -- -D warnings`: PASS.
- `cargo fmt --all -- --check`: PASS.
- The fresh real-client owner-review route completed with `passed=true` on
  seed `712816`; its final ignored artifact is
  `.analysis/seed-owner-review/20260830T204140`, and the rendered contact-sheet
  SHA-256 is `d3ac0c14bcc370a3fc25ecb83af9d586c29b0a5b95a1174a51990bb5251d9d8e`.
- The owner quality disposition remains separate from this automated and
  agent-run evidence.

Benchmark: not run. This checkpoint changes generated-column climate and
river decisions; the release-host throughput comparison remains open and no
performance conclusion is claimed.

## Evidence boundary

The artifacts provide deterministic, reviewable height, biome, and vegetation
coverage for the public-alpha worldgen checkpoint. They do not establish
subjective terrain quality, ordinary client traversal, restart persistence, or
the release-host 225-chunk throughput comparison. Those gates remain separate.
