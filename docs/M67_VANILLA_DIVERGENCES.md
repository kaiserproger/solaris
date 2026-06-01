# M67 Vanilla Behavior Divergence Review

This review separates scoped Solaris behavior from vanilla 26.1.x parity gaps.
It is not a bug list by itself: entries become M68+ work only when the gap harms
replacement-readiness or keeps code unnecessarily hard to maintain.

## Severity Key

- **High:** visible gameplay/server behavior that can confuse vanilla survival
  expectations or cause persistent state divergence.
- **Medium:** visible but scoped behavior with clear workarounds or documented
  non-parity.
- **Low:** polish, effects, exact RNG, or metadata differences that should not
  block the current scoped replacement claim.

## High Priority

| Area | Current Solaris Behavior | Vanilla Divergence | Suggested M68+ Action |
|---|---|---|---|
| Container specialization | Smokers and blast furnaces route through the current furnace storage/cooking model. Barrels use chest storage. | Vanilla has specialized timings/menus/animations and distinct block entity behavior. | Audit container abstractions first. Split shared storage from block-specific runtime before adding more container types. |
| Farming support rules | Crops, vertical plants, stems, cocoa, and saplings use local support checks and deterministic growth helpers. | Vanilla growth depends on light, soil, moisture, adjacency, age counters, RNG probability, block support, and placement survival rules. | Centralize plant support/growth policy behind named helpers and document exact local semantics per plant before adding kelp/chorus/mushrooms. |
| Deterministic drops | Crop, cocoa, sweet berry, mob, and block drops use deterministic local yields or built-in fallback loot. | Vanilla uses loot tables, fortune, RNG, tool predicates, special chances, and events. | Decide whether to expand `mc-data` loot execution or keep deterministic drops as a scoped engine feature. Avoid more ad hoc per-block tables until this decision. |
| Missing client-visible block metadata | Campfire cooking does not show item-on-campfire metadata; signs place but editing is deferred; shield pose metadata is deferred. | Vanilla clients show block/entity metadata and interactive text flows. | Prioritize oracle-backed packet/layout capture for sign editing and the most visible metadata paths. Do not guess layouts. |
| Manual survival gate | Recent M60-M66 slices passed unit/full cargo gates but not a PrismLauncher manual gate. | Replacement readiness ultimately depends on vanilla client behavior, not only unit tests. | Run a manual M67 gate before M68 planning if possible; record desyncs and missing packets as M68 items. |

## Medium Priority

| Area | Current Solaris Behavior | Vanilla Divergence | Suggested M68+ Action |
|---|---|---|---|
| Saplings and trees | Common one-by-one saplings create deterministic small Solaris-owned trees. | Vanilla has feature placement, random shapes, two-by-two trees, bees, decorators, biome rules, and exact leaf persistence behavior. | Keep deterministic trees as scoped behavior unless replacement testing shows a visible survival issue. Add larger parity only after plant helper cleanup. |
| Sugar cane, cactus, bamboo | Random ticks grow supported clear columns up to local height three. | Vanilla uses age counters, exact support/survivability checks, higher bamboo behavior, and different bonemeal rules. | Treat as local lifecycle support. If revisited, add per-plant policy rather than extending one generic vertical helper blindly. |
| Stem fruit lifecycle | Mature stems place one adjacent fruit and convert to attached stem. | Vanilla chooses positions via random checks, validates support/space rules, and has exact attached-state behavior. | Keep as unit-covered local behavior until M68 decides whether plant policy needs a wider refactor. |
| Sweet berry lifecycle | Age 2/3 bushes harvest to age 1 with deterministic yields; collision is not implemented. | Vanilla has collision damage/slowdown and randomized yields. | Collision belongs with physics/material behavior, not crop helpers. Track separately from harvest drops. |
| Cocoa lifecycle | Cocoa beans place age-0 cocoa on jungle-log sides; cocoa grows and drops deterministic beans. | Vanilla support checks, placement survivability, shape, and loot behavior are richer. | Include cocoa in the plant-support policy audit; avoid expanding support ad hoc. |
| Bows/arrows | Basic arrows support launch, local physics, block stop, lifetime despawn, pickup, entity/player damage, knockback, and simple drops/XP. | Full vanilla projectile behavior, enchantments, critical hits, potion arrows, exact collision, and sounds/events are missing. | Keep current bow scope unless combat becomes the next replacement focus. Review combat helpers for code health in M67.b. |
| Shields | Shields block frontal mob melee and arrow player damage after a delay. | Durability, axe disable, exact angle/timing, sounds/particles, metadata, and broader damage-source parity are missing. | Split shield parity into gameplay mechanics vs metadata/effects. Metadata still needs oracle-backed packet work. |
| Beds | Beds set respawn points but sleeping and time-skip are absent. | Vanilla sleep changes time/weather and enforces dimension/mob rules. | Keep respawn-only as scoped unless manual gate identifies confusion. |

## Low Priority

| Area | Current Solaris Behavior | Vanilla Divergence | Suggested M68+ Action |
|---|---|---|---|
| Particles/sounds/statistics/game events | Mostly omitted across farming, combat, containers, and plants. | Vanilla emits extensive effects and stats. | Do not prioritize until gameplay behavior is stable; add only with oracle-backed packet/event evidence. |
| Exact RNG/timing | Many systems intentionally use deterministic local behavior. | Vanilla RNG/timing differs for growth, loot, combat polish, and world events. | Preserve deterministic tests. Only introduce RNG when a replacement scenario needs it and test it with seeded boundaries. |
| World scope | Overworld survival is the current claim. | Other dimensions, portals, structures, villages/trading, redstone, boats, minecarts, weather, and full recipe book are not claimed. | Keep out of M68 cleanup unless code structure blocks future work. |

## M68 Candidates From This Review

- Design a small plant lifecycle policy layer before adding kelp, chorus,
  mushroom spread, or stricter support checks.
- Decide whether deterministic local drops remain the project policy or whether
  partial loot-table execution should move into `mc-data`.
- Audit container runtime/storage coupling before expanding smoker/blast furnace
  or barrel parity.
- Identify oracle-backed packet work needed for sign editing and high-visibility
  metadata before implementing those paths.
- Run a manual PrismLauncher pass and add any desyncs to the M68 plan.
