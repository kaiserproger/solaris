# Tellus Worldgen Study Notes

These notes summarize the local Tellus source study for Solaris M51. Tellus source
was cloned under `.analysis/tellus/Tellus` for inspection only; do not commit the
Tellus source tree, generated data, or downloaded geographic datasets.

## Source snapshot

- Repository: `https://github.com/Yucareux/Tellus`
- Local path: `.analysis/tellus/Tellus`
- Inspected target: `mc261/` plus shared `src/main/java/` code.
- Public README describes Tellus as an Earth-scale Fabric worldgen mod using
  elevation, land-cover, climate, OSM, weather, and Distant Horizons integration.

## Architecture observed

### Coordinate projection

Reference: `src/main/java/com/yucareux/tellus/worldgen/EarthProjection.java`.

- Tellus maps latitude/longitude into block coordinates through a Mercator mode by
  default, with a legacy linear mode behind `tellus.projection.mode=legacy`.
- Core constants:
  - `METERS_PER_DEGREE = 111319.49166666667`.
  - `MAX_MERCATOR_LATITUDE = 85.05112878`.
- `worldScale` means real-world meters per block. Blocks per degree is
  `METERS_PER_DEGREE / worldScale`.
- Latitude is clamped to the Mercator limit. Positive latitude maps to negative Z.

Solaris takeaway:
- Keep our first prototype deterministic/offline, but adopt the same user-facing
  concept: `world_scale_meters_per_block` and Mercator-compatible coordinate
  helpers. Do not need real lat/lon data to get the visual feel.

### Settings surface

Reference: `mc261/src/main/java/com/yucareux/tellus/worldgen/EarthGeneratorSettings.java`.

- Default world scale is `30.0` meters per block.
- Settings include terrestrial/oceanic height scale, height offset, sea level,
  spawn latitude/longitude, min/max altitude, shoreline blend distances, cave/ore
  toggles, structure toggles, Distant Horizons options, DEM selection, roads,
  buildings, and water.
- Dimension height can span a much larger range in Tellus, but Solaris should stay
  within current protocol/world assumptions until a separate height milestone.

Solaris takeaway:
- Add a small config enum first (`vanilla_like`, `tellus_like`) and only expose a
  conservative subset: scale, height scale, sea level, climate strength, and water
  toggle. Avoid copying Tellus's whole settings surface.

### Data sources and caches

References:
- `world/data/elevation/TellusElevationSource.java`
- `world/data/cover/TellusLandCoverSource.java`
- `world/data/koppen/TellusKoppenSource.java`
- `world/data/osm/*`
- `WaterSurfaceResolver.java`

Tellus relies on online/on-demand and cached data:
- DEM/elevation providers: Terrain Tiles, USGS 3DEP, Copernicus, ArcticDEM,
  SwissAlti3D, AHN, Canadian elevation, Norway DTM, Japan GSI, REMA.
- Land cover: ESA WorldCover classes.
- Climate: Koppen-Geiger codes.
- OSM: roads, buildings, water, sand; PMTiles/range readers and Overpass clients.
- Runtime caches are extensive: normalized elevation tile cache, water region
  cache, near-water chunk cache, OSM tile caches, and thread-local scratch buffers.

Solaris takeaway:
- M51 prototype must not depend on live internet. Use repo-owned noise/math and
  optional local fixtures later. Structure the code so real sidecars can be added
  as an input provider later.

### Biome placement

References:
- `EarthBiomeSource.java`
- `mc261/.../world/data/biome/BiomeClassification.java`

Observed shape:
- Raw ESA cover classes and visual cover classes drive coarse biome choice.
- Water resolves before land biome classification: ocean/river/mangrove/frozen
  terrain can short-circuit climate mapping.
- Koppen climate code is sampled/dithered, with nearest-code fallback.
- CSV mapping maps `(ESA cover class, Koppen code)` to Minecraft biome IDs.
- Cave biomes are separate and depth/noise gated.

Solaris takeaway:
- For offline Tellus-like terrain, use synthetic climate bands instead of Koppen
  files: latitude/noise -> warm/cold/dry/wet. Then map those facts onto existing
  biome IDs/facts. Keep water and frozen high-latitude/high-altitude decisions
  before general biome selection.

### Water and coast handling

References:
- `WaterSurfaceResolver.java`
- `OceanClassification.java`

Observed shape:
- Water is classified from OSM water, ESA water/no-data classes, land mask, and
  terrain surface relative to sea level.
- Water data is resolved in 64-block regions and cached.
- River/lake and ocean shorelines have blend distances, cliff slope limits, depth
  rules, and fallback paths for non-blocking generation.
- Ocean classification prefers explicit ocean hints, then land-mask-known + surface
  below sea, then no-data + surface below sea.

Solaris takeaway:
- First prototype should compute a continuous continent mask and signed distance-ish
  coastline factor from noise. Use that to smooth land/ocean transitions and avoid
  single-column water speckle. River/lake OSM parity is out of scope for the first
  offline prototype.

### Chunk generation pipeline

Reference: `mc261/src/main/java/com/yucareux/tellus/worldgen/EarthChunkGenerator.java`.

Observed shape:
- `fillFromNoise` delegates to `fillTellusSurface` and returns a completed future.
- The full chunk path builds a padded height grid (`step = 4`, grid `16 + step*2`),
  copies/interpolates chunk terrain surfaces, resolves water data, repairs terrain
  anomalies, prepares buildings, computes slope/convexity and biome caches, then
  fills sections.
- There are many performance switches: fast full chunk, two-tier surface palette,
  non-blocking terrain inputs, deferred terrain refinement, chunk detail deferral
  for roads/buildings/water/trees, per-tick apply budgets, stale prepared-state
  reaping, movement-driven prefetch.
- OSM roads/buildings use query margins and can be deferred/non-blocking.

Solaris takeaway:
- Keep generation batch-friendly: compute chunk-local height/biome/material arrays
  first, then write blocks in a tight pass. If later adding detail refinement,
  model it as bounded commands applied on the game/world path, matching Solaris's
  existing worker-result pattern.

### Roads, buildings, and structures

References:
- `world/data/osm/*`
- `worldgen/building/*`
- road/building constants in `EarthChunkGenerator.java`

Observed shape:
- Roads have classes/modes and material choices for main/normal/dirt/bridges.
- Buildings have profiles, blueprints, materials, lighting, and placement support.
- Structures and OSM details are selectively deferred and budgeted.

Solaris takeaway:
- Defer OSM roads/buildings until after terrain/climate. A Solaris-native version
  should start with synthetic roads/settlements from deterministic noise if needed,
  not live OSM.

### Runtime smoothness lessons

Tellus contains many toggles aimed at avoiding chunk stalls:
- non-blocking terrain input modes
- prefetch around chunks and movement direction
- deferred detail refinement
- per-tick apply budgets
- stale prepared-state reaping
- cache sizes and thread-local scratch buffers

Solaris takeaway:
- The Tellus-inspired generator should emit coarse terrain synchronously and push
  optional details through bounded worker/result queues. Avoid blocking chunk
  workers on external IO.

## Solaris prototype design

Initial offline `tellus_like` mode should implement:

1. Projection/settings helpers:
   - `world_scale_meters_per_block` defaulting near 30.
   - Optional spawn lat/lon later; for now map chunk coords directly into a
     synthetic Earth-like plane.
2. Height model:
   - Low-frequency continent mask for oceans vs continents.
   - Medium-frequency ridge/mountain noise.
   - Separate oceanic and terrestrial height scales.
   - Sea level clamp and smooth coastline blend.
3. Climate/biome facts:
   - Synthetic latitude bands from Z coordinate.
   - Temperature falls with altitude.
   - Moisture noise chooses plains/forest/desert/snowy/frozen/ocean variants.
4. Chunk pipeline:
   - Build arrays for `height[16*16]`, `water[16*16]`, `surface_kind[16*16]`.
   - Fill blocks after facts are complete.
   - No live network or blocking file IO.
5. Tests:
   - Projection round trips/clamps.
   - Seam continuity across chunk boundaries.
   - Height bounds and sea-level water fill.
   - Climate changes with latitude and altitude.

## Explicit non-copying rule

These notes are design guidance. Solaris implementation must use repo-owned Rust
math/data and should not paste Tellus Java code or vendor Tellus resource files.
