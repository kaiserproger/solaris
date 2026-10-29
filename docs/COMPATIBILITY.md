# Vanilla compatibility — Minecraft Java 26.1.2

**Draft prerelease; not a drop-in vanilla replacement.** This matrix describes
`solaris-v2` (`mc-edge` + `mc-core`) at the **integration candidate**, reviewed 2026-10-23.
It distinguishes implemented mechanics from a complete, naturally obtainable
survival experience. Native strongholds, fortresses and End cities now generate;
that does **not** certify a complete fresh-world actual-client survival route.

## Summary

Counts are feature rows below, not percentages of Minecraft, tests or mobs.
A broad `partial` row must not be counted as fully working.

| Status | Rows |
|---|---:|
| `works` | 6 |
| `partial` | 95 |
| `missing` | 1 |
| `n/a` | 2 |
| **Total** | **104** |

- **`works`**: the explicitly bounded behavior has retained execution evidence;
  this does not certify adjacent features or every input.
- **`partial`**: implementation exists, but a concrete parity gap or a stated
  evidence gap remains. Lack of a full gate is not proof the feature is absent.
- **`missing`**: the named runtime behavior is absent, not merely untested.
- **`n/a`**: a Solaris/client extension, not a vanilla-server parity requirement.
- **tested with real client — RC1** marks the retained native Java-client pass
  described below. **RC-H** marks historical real-client evidence only; it is
  explicitly not native-runtime acceptance. **RC-G** means a real-client gate
  exists but a current pass was not established. Protocol bots, unit tests,
  vanilla-server oracles and a prepared manifest are not real-client passes.

A test-file citation means coverage exists in source, **not that this review ran
it**. File/line references are for the pinned tree. The October 23 audits were
static inspections of an earlier tree (`286c275c`); their reproduction scenarios
were not executed by those auditors. [Superseded findings](#audit-provenance-and-superseded-findings)
are separated from open gaps. Unintegrated worker patches are not support claims.
See [DoD](DEFINITION_OF_DONE.md), [architecture](ARCHITECTURE.md) and
[the historical ledger's runtime warning](VALIDATION_LEDGER.md#L1-L8).

Current server-fixes behavior (source and focused regression scope, not live acceptance):

- Command-driven container destruction settles drops, storage custody and viewer
  closure before another input can click the removed inventory.
- Hoppers pulling from container vehicles refresh their open viewer inventories.
- Restored due gravity ticks activate unsupported falling blocks without requiring
  a neighboring edit.
- Iron-ingot interaction repairs a living iron golem by 25 health, capped at its
  maximum; only a successful survival repair consumes an ingot. Both hands,
  Creative repair and full-health no-op have focused regression coverage.
- Mob melee damage scales with difficulty for player victims, including attacks
  by natural iron golems; attacks on nonplayer mobs retain their base damage.
- Without plugin-login arbitration, a newer same-UUID login disconnects the old
  session and transfers its saved player state rather than duplicating inventory.
- Play pick-item packets select or move an existing survival stack; Creative can
  create the picked block item or entity spawn egg. Reach and reservation-floor
  admission still apply. See [pick regressions](../crates/mc-core/src/tick/input/pick_item_tests.rs).
- Torchflower crops and pitcher crops have random growth; torchflowers accept
  dispenser bone meal. Crop, propagule and bell support is checked on neighboring
  block changes. These changes have focused regression cases but no new
  real-client acceptance.
- Legacy creative inventory updates matching a sent 26.3-only item stand-in are
  ignored to preserve the real item; other replacements still apply. Identical
  stand-in selections in that same slot cannot be distinguished from echoes.
  This has focused translation coverage, not a new real-client gate.

## Protocol, login, chat and commands

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| 26.3 protocol (777), legacy 26.1.2 (775), status ping, Configuration and Play | `partial` | [Protocol translation contract and 2026-10-28 agent-run graphical evidence](decisions/0015-multiversion-protocol-translation.md): canonical 777 Java `core-client` (Play/chat subset), `inventory`, `survival` and `entities` passed on one release binary; earlier 775 survival and inventory/cursor/crafting profiles also passed; [legacy table/layout regressions and TCP join/chunk gate](../crates/mc-edge/src/legacy/); [keepalive regressions](../crates/mc-edge/src/server_session_security_tests.rs) | Legacy sessions require a separate 26.1.2 content cache; new blocks/items/entities are polyfilled by name. These bounded passes do not certify every packet or complete parity. Clients logged nonfatal trial-spawner NBT registry errors (775) and decorated-pot `sherds` NBT errors (777); see the ADR's exact receipts and evidence limits. |
| Offline login identity | `partial` | [login identity](../crates/mc-edge/src/login.rs); [atomic reservations and retired-stash regressions](../crates/mc-edge/src/server_session_security_tests.rs); [private-custody/competing-claim regressions](../crates/mc-core/src/auth_gate_tests.rs); [queued Leave regression](../crates/mc-core/src/core_session_tests.rs) | Offline UUID intentionally derives from a chosen name, not verified ownership. Native fixes address [AUTH-01]/[AUTH-02]/[AUTH-03] and private saved-state disclosure; optional password-plugin AUTH-04/05 remains owned by the sibling auth package. No new live password-plugin gate. |
| Online mode / Mojang authentication | `partial` | [login.rs:174–243](../crates/mc-edge/src/login.rs#L174-L243): encryption challenge, session verifier, verified UUID, failure disconnect; shared lifecycle fixes and tests above | Fail-closed verifier exists; no retained live Mojang-account gate used here. Signed chat certificate/chain verification is not supplied by online login. |
| Velocity modern forwarding | `partial` | [login.rs:132–173](../crates/mc-edge/src/login.rs#L132-L173): verified forwarding and name agreement | Trusted-proxy path, not protocol translation; no end-to-end proxy/client receipt established here. |
| Plain chat, private messages, `/me`, `/say`, signed commands | `partial` | [signed/unsigned command decode regressions](../crates/mc-edge/src/decode_tests.rs); [cross-dimension message/team regressions](../crates/mc-core/src/dimensions_tests.rs); [spam/mute and unsigned-chat policy](OPERATING.md#L431-L463) | Signed command envelopes now enter native permission/mute/spam admission ([SC-03]); commands retain their own 32,767-unit limit ([SC-05]). `/teammsg` and `/tm` deliver to authenticated team members across dimensions. Signature/certificate verification and reportable signed-chat delivery remain absent; no new real-client chat gate. |
| Operator/gameplay commands, selectors, `/execute`, teams, scoreboard, titles, bossbars | `partial` | [execution context regressions](../crates/mc-core/src/command_context_tests.rs); [data/store regressions](../crates/mc-core/src/command_data_tests.rs); [automatic criteria and team visibility regressions](../crates/mc-core/src/tick/damage_scoreboard_tests.rs); [operating contract](OPERATING.md#datapacks-and-command-blocks) | Native scoreboard criteria include health/absorption, food, air, armor, XP, level, death/player/total kill counts and team-color kill counts. Team death-message visibility persists through scoreboard NBT and native snapshots. `/execute` source modifiers, entity/block/score conditions, and `store result\|success score` use native command outcomes for the queries and mutators listed in the operating contract. Unsupported producers are rejected before execution. Other store destinations, `blocks`, block tags/SNBT predicates and rich suggestions remain absent. `/locate` only finds generated villages; bossbar Anvil export is unwired. [SC-05]. |
| `/summon` age and baby NBT | `partial` | [family regressions](../crates/mc-core/src/command_summon_age_tests.rs); [existing summon tests](../crates/mc-core/src/command_summon_tests.rs) | `Age` follows vanilla ageable families, including squid and undead horses; frog/parrot/camel-husk/zombie-nautilus retain age without a baby model. `IsBaby` supports the zombie family, piglin and zoglin as boolean state, not a growth clock. Villager and tadpole retain their existing age owners. Original model-gallery real-client replay is still required; unit/checkpoint coverage is not visual acceptance. |
| Map-making commands: `/clone`, `/data`, `/function`, `/datapack`, `/schedule`, `/place`, `/fillbiome`, `/damage`, `/recipe`, `/trigger`, `/tag`, `/spreadplayers`, `/rotate` | `partial` | [datapack runtime/tests](../crates/mc-core/src/datapacks_tests.rs); [data query/store tests](../crates/mc-core/src/command_data_tests.rs); [damage command regressions](../crates/mc-core/src/command_damage_tests.rs); [operating limits](OPERATING.md#datapacks-and-command-blocks) | `/function`, `/schedule`, `/datapack list`, `/reload`, `/data get entity/block` with NBT paths/scale, and restricted command-block `/data merge block` implemented. `/damage` uses one target, finite nonnegative amount, optional supported damage type, and `at` or `by`/`from` attribution through native damage admission; unmapped types fail before mutation rather than falling back to generic damage. Command permission/tree, hit feedback and attribution have regression coverage, not an original-path live receipt. `/damage` is not an execute-store result producer and is rejected under store before mutation. Other listed roots and general entity/storage NBT editing remain absent. |
| `/item`, `/loot`, `/attribute` | `partial` | [command execution regressions](../crates/mc-core/src/command_execution_tests.rs) | Item replacement supports ordinary entity equipment/player inventory and container slots; modifiers and slot sources are not implemented. Loot uses chest catalogs or mob kill context; mine/fish contexts are refused. Attribute commands support retained mob and player attributes for the implemented kinds; unsupported kinds are refused. Player `scale` and `step_height` are sent to clients but do not change server collision; actual-client parity remains unverified. |
| Command blocks: impulse, chain, repeating | `partial` | [native scheduled execution](../crates/mc-core/src/command_blocks.rs); [regressions](../crates/mc-core/src/command_blocks_tests.rs); [configuration and limits](OPERATING.md#datapacks-and-command-blocks) | Creative operator placement; disabled by default. Impulse, chain and repeating blocks execute native commands, with conditional checks against the block behind. Configure through `/data`, not the client editor. Comparator success output, command minecarts and editor packets remain absent. |
| Books/quills: editing and signing | `partial` | [native edit/sign tests](../crates/mc-core/src/books_tests.rs); [book packet tests](../crates/mc-protocol/src/packets/play/book_packet_tests.rs); [component codecs](../crates/mc-protocol/src/packets/play/copied_components/book_tests.rs) | EditBook now enters native admission and persists pages/title/author; written books open through edge projection ([SC-06] implementation gap closed). Actual-client editing/signing and filtered/styled imported-book coverage remain unproven. |

Selector `dx`/`dy`/`dz` volumes intersect entity body bounds, not just entity
positions. Once an extent is present, omitted extents are zero; signed/fractional
extents retain the one-block upper expansion, and merely touching a face does
not intersect. Distance filters still test the entity position and combine with
the volume filter. This is a narrow selector implementation change, not complete
selector parity or a real-client acceptance claim: option coverage, dimension
scope, dead players, declared target types and random evaluation remain open.

Saved-map implementation is **partial** ([native owner](../crates/mc-core/src/maps.rs),
[regressions](../crates/mc-core/src/maps/tests.rs)): empty-map creation, bounded
resident-world color sampling, hand/offhand and viewed item-frame packets,
banner markers, cartography clone/scale/lock, native WAL/checkpoints and Anvil
map interchange are wired. Exploration loot and cartographer offers resolve
actual generated/saved structure destinations through the existing locate
worker. No real-client map-rendering acceptance is claimed here. Sampling uses
dominant vanilla block colors with a normal shade rather than exact vanilla
height/water shading; unloaded terrain remains unexplored.

## Movement, physics and combat

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Walking, sprinting, jumping, collision, stairs and fall damage | `partial` | [movement owner](../crates/mc-core/src/tick/input/movement.rs); [landing/water regressions](../crates/mc-core/src/tick/movement_tests.rs); [mining support regression](../crates/mc-core/src/tick/mining_tests.rs#L433); **tested with real client — [RC1](#real-client-evidence)**, supported movement/mining only | Native landing now uses real support, hay/honey/bed/slime damage factors and flowing/contained-water fall cancellation (VS-08 damage and VS-09). Bounce/landing client feel remains unvalidated; connected-block geometry is tracked separately ([VB-26]). |
| Diagonal sprint speed acceptance (bounded oracle scenario) | `works` | [26.1.2 player oracle result](QUIRK_PARITY.md#L113-L115): zero corrections on both servers | Only the named diagonal-sprint scenario, not complete movement or graphical-client proof. |
| Water swimming, buoyancy, drowning, bubble-column transport | `partial` | [movement/eye-fluid regressions](../crates/mc-core/src/tick/movement_tests.rs); [fluid surface sampling](../crates/mc-core/src/fluids.rs); [RC-G](#real-client-evidence) | Pose-dependent drowning samples fluid height; water movement uses three-dimensional swimming exhaustion rather than land sprint costs (VS-10, VS-16). Bubble-column transport/generation and the broad swim-feel client gate remain absent/unproven respectively. |
| Elytra/gliding and firework boost | `partial` | [movement owner](../crates/mc-core/src/tick/input/movement.rs); [elytra_tests.rs](../crates/mc-core/src/elytra_tests.rs); [rocket attachment/custody tests](../crates/mc-core/src/projectiles/weapons_tests.rs); [generated ship reward custody/restart tests](../crates/mc-core/src/villager/generated_tests.rs) | Landing requires actual support, not an airborne ground-flag claim. Naturally generated ships carry obtainable elytra; native coverage is not a full exploration/flight/landing client gate. |
| Boats, minecarts, mounts, passenger travel | `partial` | [vehicles_tests.rs](../crates/mc-core/src/vehicles_tests.rs); [mounts_tests.rs](../crates/mc-core/src/mounts_tests.rs); [empty-vehicle portal regression](../crates/mc-core/src/portals_tests.rs) | Empty boats/minecarts now use the ten-tick portal cooldown (W12). Rail connection/power shape ([VB-08], [VB-09]), atomic mounted portal travel ([W09]) and horse/donkey breeding ([VC-13]) remain separately tracked. |
| Melee: cooldown, critical hits, sweep, armor, enchantments, knockback, mace | `partial` | [combat_tests.rs](../crates/mc-core/src/combat_tests.rs); [mace_tests.rs](../crates/mc-core/src/mace_tests.rs); [native parity regressions](../crates/mc-core/src/tick/combat_parity_tests.rs) | VC-03/04 and VS-01 have focused fixes: retained fall distance plus water/climbing/mount exclusions, current mob melee sight admission, and target mode immunity. No broad native real-client combat pass established. |
| Shields, bows, crossbows, tridents and projectile deflection | `partial` | [shield admission](../crates/mc-core/src/tick/damage.rs); [weapon tests](../crates/mc-core/src/projectiles/weapons_tests.rs); [native parity regressions](../crates/mc-core/src/tick/combat_parity_tests.rs) | VC-01/02/08/18 have focused fixes: retained blast origin, actual arrow Piercing, player-crossbow critical arrows, and merge-time shield rebound before arrow removal. Broad client weapon/deflection acceptance remains unestablished. |
| Eye of ender guidance | `partial` | [native use/flight](../crates/mc-core/src/projectiles/eye.rs); [custody, lifecycle and retained-state tests](../crates/mc-core/src/projectiles/eye_tests.rs); [shared bounded structure lookup](../crates/mc-core/src/command_locate.rs) | Air use launches toward a found stronghold; consumption waits for a valid native-tick result. Retained target/age, four-second flight and drop/shatter outcomes exist. Smooth rise/hover is not the exact oscillating trajectory; no real-client guidance gate. Generated stronghold availability is owned by [W01]. |
| Explosions, fire, lava, suffocation, falling-object hazards | `partial` | [contact_hazards_tests.rs](../crates/mc-core/src/contact_hazards_tests.rs); [falling_tests.rs](../crates/mc-core/src/falling_tests.rs); [shield regression](../crates/mc-core/src/tick/combat_parity_tests.rs) | Falling anvils/stalactites queue bounded native impact damage (VB-17); lava afterburn retains 300 ticks (VS-19), and exposed rain extinguishes actors (VS-11). Helmet wear, broad hazard-client parity and fire spread/waterlogging (VB-28/29) remain unproven. |

## Mobs by family and villagers

A registry entry, spawn egg or `species_tests` lifecycle pass is not proof of a
mob's complete AI. The [species baseline list](../crates/mc-core/src/species_tests.rs#L10-L83)
and [spawn/move/death test](../crates/mc-core/src/species_tests.rs#L145-L218)
cover broad representation; family-specific gaps remain below. No unlisted
species is implicitly certified.

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Natural spawning, mob caps, despawn and loot/XP | `partial` | [natural_spawn_parity_tests.rs](../crates/mc-core/src/natural_spawn_parity_tests.rs); [natural_spawn_breadth_tests.rs](../crates/mc-core/src/natural_spawn_breadth_tests.rs); [entity_loot_tests.rs](../crates/mc-core/src/entity_loot_tests.rs); [ordinary spawners](../crates/mc-core/src/monster_spawner_tests.rs) | Spawn-proof glass/bottom slabs, baby-monster loot and natural-equipment XP now have regression coverage ([VC-05], [VC-09], [VC-20]); farm kill-chamber drift remains [documented](QUIRK_PARITY.md#L124-L126). Ordinary spawners support bounded single-type monster configurations; weighted/custom NBT fails admission. |
| Farm animals: cows/mooshrooms, pigs, sheep, chickens, rabbits; breeding | `partial` | [entity_life_tests.rs](../crates/mc-core/src/entity_life_tests.rs); [food temptation and juvenile following](../crates/mc-core/src/tick/passive_follow.rs) | VC-06 has focused native regressions for both-hand food temptation and baby following. Direct steering still does not provide complete obstacle navigation; lifecycle coverage is not full species/client behavior. |
| Chicken egg production rate (bounded oracle farm) | `works` | [egg RNG fix and passing farm receipt summary](QUIRK_PARITY.md#L106-L115) | Only `farm-chicken-eggs`, not all chicken AI, breeding or farm layouts. |
| Tameable/neutral animals: wolves, cats, parrots, foxes, ocelots | `partial` | [taming tests:4,44,97,149](../crates/mc-core/src/taming_tests.rs#L4); [blueprint-driven food following](../crates/mc-core/src/tick/passive_follow.rs) | Ownership/sitting/trust and food-following coverage is narrower than owner follow/defense, teleport, shoulder and complete species behavior; no broad native client gate. |
| Mounts: horses/donkeys/mules, llamas, camels, striders, happy ghasts | `partial` | [mounts_tests.rs](../crates/mc-core/src/mounts_tests.rs); [horse/donkey courtship and mule regression](../crates/mc-core/src/entity_life_tests.rs); [pair admission](../crates/mc-core/src/entity_life_breeding.rs) | VC-13 has a focused native fix for both parent orders with sterile offspring. Mounted portal transfer remains [W09]; happy-ghast riding is absent from mount admission, and generic lifecycle is not special mount parity. |
| Other terrestrial passives: pandas, polar bears, goats, armadillos, sniffers | `partial` | [species list:19–23](../crates/mc-core/src/species_tests.rs#L19-L23), [lifecycle:146](../crates/mc-core/src/species_tests.rs#L146) | Baseline movement/breeding/death coverage only; no complete per-species special-behavior or real-client gate established. No dedicated audit ID. |
| Aquatic/amphibious: fish, squid/glow squid, dolphins, nautiluses, axolotls, frogs, turtles, tadpoles | `partial` | [species list:38–49](../crates/mc-core/src/species_tests.rs#L38-L49); [dolphin grace:69](../crates/mc-core/src/dolphin_tests.rs#L69), [item play:171](../crates/mc-core/src/dolphin_tests.rs#L171) | Dolphin-specific tests do not establish parity for every aquatic species; broad navigation, breeding and client presentation remain unproven. |
| Bees and beehives | `partial` | [nectar/sting:283](../crates/mc-core/src/bees_tests.rs#L283); [hive WAL custody:332](../crates/mc-core/src/bees_tests.rs#L332), [silk-touch custody:574](../crates/mc-core/src/bees_tests.rs#L574) | Native action/persistence tests, not complete pollination/pathing/farm or real-client acceptance. No dedicated open audit ID. |
| Allays | `partial` | [collection/delivery:86](../crates/mc-core/src/allay_tests.rs#L86); [partial-pickup recovery:6](../crates/mc-core/src/allay_tests.rs#L6) | INT-02 has a focused recovery fix; broad delivery/navigation/client parity still unproven. See [superseded findings](#audit-provenance-and-superseded-findings). |
| Undead walkers: zombies, husks, drowned and newer variants | `partial` | [hostile_tests.rs](../crates/mc-core/src/hostile_tests.rs); [baby loot/equipment XP](../crates/mc-core/src/entity_loot_tests.rs); [authored inhabitants](../crates/mc-core/src/villager/generated_tests.rs) | Baby loot and natural-equipment XP corrected ([VC-09], [VC-20]); melee admission checks intervening geometry ([VC-04]); blocked direct pursuit remains ([VC-15]). |
| Skeleton archers: skeletons, strays, bogged, parched; wither skeletons | `partial` | [hostile.rs](../crates/mc-core/src/hostile.rs); [hostile_tests.rs](../crates/mc-core/src/hostile_tests.rs); [equipment XP](../crates/mc-core/src/entity_loot_tests.rs) | Equipped-mob XP corrected ([VC-20]); broad pathing/loot parity and airborne farm drift remain ([quirk findings](QUIRK_PARITY.md#L124-L126)). |
| Creepers | `partial` | [native fuse and item-use admission](../crates/mc-core/src/hostile.rs); [manual ignition/debit/retention regression](../crates/mc-core/src/hostile_tests.rs); [combat regressions](../crates/mc-core/src/tick/combat_parity_tests.rs) | Flint-and-steel/fire-charge ignition now survives target departure and persists ([VC-14]); shield admission retains explosion origin ([VC-01]). Broad actual-client fuse/combat parity remains unproven. |
| Spiders/cave spiders, silverfish, endermites | `partial` | [hostile_tests.rs](../crates/mc-core/src/hostile_tests.rs); [ordinary spawner tests](../crates/mc-core/src/monster_spawner_tests.rs) | Spider acquisition and retained-target refresh now share local-light/recent-retaliation admission ([VC-07]). Other arthropod special behavior is not fully gated. |
| Slimes and magma cubes | `partial` | [entity_loot.rs split path](../crates/mc-core/src/entity_loot.rs); [species lifecycle](../crates/mc-core/src/species_tests.rs#L146) | Walker-style motion instead of hopping ([VC-19]); split/death coverage does not fix locomotion. |
| Endermen | `partial` | [enderman_tests.rs](../crates/mc-core/src/enderman_tests.rs); [combat audit positive checks][combat-audit] | Dry supported teleport/projectile immunity/pumpkin/water checks exist; full stare, carrying, navigation and client cadence unproven. |
| Witches | `works` | [potion selection and drinking](../crates/mc-core/src/special_mobs/shooters.rs); [special_mobs_tests.rs](../crates/mc-core/src/special_mobs_tests.rs); [magic resistance regression](../crates/mc-core/src/entity_loot_tests.rs) | Native potion combat: magic/poison/thorns resistance and close-range probabilistic Weakness throws close [VC-10], [VC-17]. This is not a real-client or witch-farm yield oracle. |
| Piglins/brutes, zombified piglins, hoglins/zoglins; bartering | `partial` | [accepted barter:75](../crates/mc-core/src/piglin_tests.rs#L75); [authored native inhabitants](../crates/mc-core/src/villager/generated_tests.rs); [Nether structures](../crates/mc-worldgen/src/nether/structures.rs) | Bastion inhabitants use blueprint-backed once-only claims and authored persistence. Five Nether habitats now generate; complete anger, hunting and transformation parity remains unproven. |
| Blaze and ghast ranged mobs | `partial` | [blaze/ghast combat](../crates/mc-core/src/special_mobs_tests.rs); [ordinary monster spawners](../crates/mc-core/src/monster_spawner_tests.rs); [fortress contents](../crates/mc-worldgen/src/nether/content_tests.rs) | Generated fortresses supply blaze spawners; native activation checks range/delay/light/space/nearby caps and retains state through WAL. Recurring fortress spawn-table overrides and full flight/client parity remain outside this gate. |
| Guardians/elder guardians, shulkers, breeze | `partial` | [special attacks](../crates/mc-core/src/special_mobs_tests.rs); [authored elder-guardian/shulker inhabitants](../crates/mc-core/src/villager/generated_tests.rs); [modern structure contract](VILLAGE_GENERATION.md#modern-jigsaw-structures) | Monument and End-city inhabitants have native once-only identity/persistence; trial chamber templates now generate. Complete naturally generated encounters remain unverified; custom equipment/attachments are not covered by generic mob markers. |
| Illagers/raiders: pillagers, vindicators, evokers, vexes, ravagers; raids | `partial` | [native raids](../crates/mc-core/src/raids_tests.rs); [authored pillager/vindicator/evoker inhabitants](../crates/mc-core/src/villager/generated_tests.rs); [classic structures](../crates/mc-worldgen/src/overworld_structures/catalog.rs) | Outposts and mansions generate with structure-native inhabitants using the blueprint/claim path. Complete vanilla-client raid/navigation/farm acceptance remains unproven. |
| Warden/sculk and creaking | `partial` | [warden anger:82](../crates/mc-core/src/warden_tests.rs#L82), [sonic:393](../crates/mc-core/src/special_mobs_tests.rs#L393); [creaking gaze:33](../crates/mc-core/src/creaking_tests.rs#L33) | No full naturally generated encounter gate; underground sculk features absent ([W08]), pale-oak growth absent ([VB-23]). |
| Iron/snow/copper golems | `partial` | [golems_tests.rs](../crates/mc-core/src/golems_tests.rs); [copper_tests.rs](../crates/mc-core/src/copper_tests.rs); [iron loot oracle](QUIRK_PARITY.md#L109-L115) | Gossip summoning now admits five willing villagers; panic requires three. Detection uses the 16-block per-axis volume. Iron-ingot healing and attack damage/difficulty differ ([VC-11], [VC-16]); real-client summoning proof remains pending. |
| Wither boss | `partial` | [ritual, restart, heads, armor, grief, star:626](../crates/mc-core/src/special_mobs_tests.rs#L626); [LOS/immunity:988](../crates/mc-core/src/special_mobs_tests.rs#L988); [Nether structures](../crates/mc-worldgen/src/nether/structures.rs) | Native focused tests, not complete real-client boss acceptance. Fortress resources now generate, but recurring fortress-specific wither-skeleton spawning and a full natural skull-acquisition route remain unproven ([W02]). |
| Bats/phantoms and command-only giant/illusioner | `partial` | [species list](../crates/mc-core/src/species_tests.rs#L10-L83); [giant:64](../crates/mc-core/src/rare_mobs_tests.rs#L64), [illusioner:136](../crates/mc-core/src/rare_mobs_tests.rs#L136) | Generic lifecycle and named special tests do not certify bat/phantom AI. Giant/illusioner command-only availability is not itself a vanilla gap. |
| Villagers: jobs, beds, POIs, schedules, breeding, gossip, golems | `partial` | [villager/brain.rs](../crates/mc-core/src/villager/brain.rs); [villager persistence tests](../crates/mc-core/src/villager/persistence_tests.rs); [combat audit][combat-audit] | Gossip-triggered golem eligibility is implemented ([VC-12]); farmer harvest about 2.4× vanilla in [breeder oracle](QUIRK_PARITY.md#L127). Settlement residency is an extension, not vanilla proof. |
| Villager/wandering-trader trade UI, offers, restock and prices | `partial` | [villager/trades_tests.rs](../crates/mc-core/src/villager/trades_tests.rs); [merchant_tests.rs](../crates/mc-core/src/tick/merchant_tests.rs); [trader persistence](../crates/mc-core/src/level_metadata_trader_tests.rs) | Native trading/restock coverage; complete vanilla offer/demand/discount/UI and survival acquisition gate not established. No dedicated audit ID. |

## Items, stations and containers

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Mining, tool speed/durability, item drops and placement | `partial` | [mining tests](../crates/mc-core/src/tick/mining_tests.rs); [sneak-placement tests](../crates/mc-core/src/chests_tests.rs); **tested with real client — [RC1](#real-client-evidence)**: grass break, dirt pickup/place and debit | VI-01 timed admission, VI-12 zero-hardness tool exemption and VI-11 container sneak-placement implemented. Shape/support gaps and broad client validation remain; [item closure report][items-gap]. |
| Item stacking, inventory/cursor custody, pickup and drop | `partial` | [inventory/click_tests.rs](../crates/mc-core/src/inventory/click_tests.rs); [player_tests.rs](../crates/mc-core/src/player_tests.rs); **tested with real client — [RC1](#real-client-evidence)**, one dirt stack | Focused INT-01..05 fixes do not certify every malformed click, component or restart interaction; see [closure notes](#audit-provenance-and-superseded-findings). |
| Shaped/shapeless crafting, repair and recipe book | `partial` | [crafting custody regressions](../crates/mc-core/src/inventory/crafting_tests.rs); [crafter copying test](../crates/mc-core/src/crafter_tests.rs); [recipe_book_tests.rs](../crates/mc-core/src/inventory/recipe_book_tests.rs); [map owner/tests](../crates/mc-core/src/maps/tests.rs); [RC-G](#real-client-evidence), oak-log recipe only | Dynamic book/banner copies retain their source; repair merges curses and accepts omitted zero damage (VI-06/14). Native saved-map extension and book wire codecs are integrated. VI-04 remains partial: generic special-serializer import and other special recipes are not complete; broad recipe/client gate remains. |
| Furnaces, blast furnaces, smokers and campfires | `partial` | [furnace owner regressions](../crates/mc-core/src/tick/furnace_tests.rs); [inventory/furnace_tests.rs](../crates/mc-core/src/inventory/furnace_tests.rs); [campfire_tests.rs](../crates/mc-core/src/campfire_tests.rs) | Recipe-book shared-input placement, durable recipe-count XP banks and destruction XP implemented (VI-08/09/10); broad station-client gate remains. [Item closure report][items-gap]. |
| Brewing and potion batches | `partial` | [brewing recipes, cancellation and checkpoint tests](../crates/mc-core/src/brewing_tests.rs); [fortress wart/blaze supply](../crates/mc-worldgen/src/nether/content_tests.rs); [ordinary spawner activation](../crates/mc-core/src/monster_spawner_tests.rs) | Generated fortresses supply wart gardens and blaze spawners, feeding the native brewing recipes/timing/custody path. Full fresh-world ingredient acquisition and every potion/UI path remain unverified. |
| Enchanting table and enchantment effects | `partial` | [inventory/enchanting_tests.rs](../crates/mc-core/src/inventory/enchanting_tests.rs); [stable-offer regression](../crates/mc-core/src/player/stations_tests.rs); [enchantment_tick_tests.rs](../crates/mc-core/src/enchantment_tick_tests.rs) | VI-07 gameplay RNG is separated from persisted enchanting offers. Starvation bypasses Protection and retained arrow state governs piercing ([VS-07], [VC-02]); full client station coverage remains unproven. |
| Anvil, smithing, stonecutter and grindstone | `partial` | [inventory/anvil.rs:42–171](../crates/mc-core/src/inventory/anvil.rs#L42-L171); [craft_stations_tests.rs](../crates/mc-core/src/inventory/craft_stations_tests.rs); [player station tests](../crates/mc-core/src/player/stations_tests.rs) | Bounded native repair/merge/cost/station transactions; no full actual-client station matrix. Falling-anvil damage/helmet wear is tracked separately in the falling-object row (GapPhysics). |
| Food, drink, effects and item remainders | `partial` | [consumables_tests.rs](../crates/mc-core/src/consumables_tests.rs); [effects_tests.rs](../crates/mc-core/src/effects_tests.rs); [item closure report][items-gap] | VI-05 harmful foods now apply guaranteed/probabilistic effects at successful consumption, without rerolling enchanting offers. Broad native-client food/drink/effect combinations remain unvalidated. |
| Chest/double chest, barrel, shulker box, shared menus and drag/shift clicks | `partial` | [chests_tests.rs](../crates/mc-core/src/chests_tests.rs); [shulker_tests.rs](../crates/mc-core/src/shulker_tests.rs); **tested with real client — [RC-H](#real-client-evidence)**, historical shared-chest routes only | Native sneak-use and block-lid gates implemented (VI-11/13), using the shared redstone conductor predicate and shulker facing/half-block clearance. Sitting-cat obstruction and broader shape/entity/client combinations remain. INT-01/04 fixed; historical client evidence does not revalidate native containers. |
| Vanilla structure chest loot catalog | `partial` | [startup compiler](../crates/mc-data/src/loot/chest_26_1_2.rs), [functions](../crates/mc-data/src/loot/chest_functions.rs), [regressions](../crates/mc-data/src/loot/chest_26_1_2_tests.rs) | Loads all 56 vanilla chest tables, 6 archaeology tables and 15 hero gifts from the selected content root. Supports nested trial rewards, ordered functions/conditions, random and level enchantments, potions, stew, horns and preserved text/components. Unknown functions/conditions fail at load with table/path diagnostics. Exploration-map tables require the native saved-map callback: missing context fails explicitly without consuming RNG; a completed lookup with no destination omits the map. Callers retain unresolved table/seed until first-open lookup completes. Fishing retains its separate context-aware evaluator; entity/tool/explosion conditions are not silently accepted as chest contexts. This is compiler/native-stack coverage, not real-client or structure-placement proof. |
| Ender-chest personal inventory | `partial` | [personal inventory tests](../crates/mc-core/src/ender_chest_tests.rs); [native owner](../crates/mc-core/src/ender_chest.rs) | Private 27-slot custody is retained in player `EnderItems`, with shared click planning and fail-closed recovery ([VI-02] implementation gap closed). Actual-client/reconnect/dimension/death matrix and lid animation/audio remain unproven. |
| Item frames/glow item frames | `partial` | [native lifecycle/recovery tests](../crates/mc-core/src/item_frame_tests.rs); [edge spawn tests](../crates/mc-edge/src/plan/books_frames_tests.rs) | Placement, displayed-item custody, rotation, attack/support drops and native recovery implemented ([VI-03]). Map-decoration integration, `Fixed`/`ItemDropChance`, special non-full support and actual-client display remain gaps. |
| Armor stands | `partial` | [armor_stand_tests.rs](../crates/mc-core/src/armor_stand_tests.rs); [armor_stand.rs:60–183](../crates/mc-core/src/armor_stand.rs#L60-L183) | Placement/equipment/double-hit behavior has native coverage; complete damage/NBT/client combinations unproven. |
| Signs/hanging signs, beds and campfire block entities | `partial` | [signs_tests.rs](../crates/mc-core/src/signs_tests.rs); [sleep_tests.rs](../crates/mc-core/src/sleep_tests.rs); [RC-G](#real-client-evidence), regular-sign probe only | Broad hanging/waxed/styled text, restart and client visuals unproven; bed fractional-shape exits remain approximate (see survival rows). |
| Fishing, buckets/bottles, beacons, conduits, jukeboxes, trial spawners and vaults | `partial` | [fishing tests](../crates/mc-core/src/projectiles/fishing_tests.rs), [beacon](../crates/mc-core/src/beacon_tests.rs), [conduit](../crates/mc-core/src/conduit_tests.rs), [trial spawner](../crates/mc-core/src/trial_spawner_tests.rs), [vault](../crates/mc-core/src/vault_tests.rs) | Focused native mechanics, not all item use or full survival encounters. No generated trial chambers/monuments; waterlogged interaction gaps [VB-13], [VB-14]. |

## Blocks, redstone, fluids, light and farming

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Placement/support, doors/trapdoors, fences/walls/panes and stairs | `partial` | [placement_tests.rs](../crates/mc-core/src/placement_tests.rs); [placement_shapes.rs](../crates/mc-core/src/placement_shapes.rs) | Native regressions cover diode floor loss ([VB-24]), torch fence/wall supports ([VB-25]), connection updates ([VB-26]) and paired-door hinge choice ([VB-30]). Wall height/post and obstruction geometry use bounded approximations; full shape/client matrix remains unproven. |
| Redstone dust, solid-block conduction, levers/buttons/plates, torches and lamps | `partial` | [signal.rs](../crates/mc-core/src/redstone/signal.rs), [gap regressions](../crates/mc-core/src/redstone/gap_tests.rs), [redstone_tests.rs](../crates/mc-core/src/redstone/redstone_tests.rs) | Native strong/weak one-block conduction, directional dust and indirect consumer wakes address [VB-02], [VB-03]. Torch burnout and exact update-order parity remain outside this implementation. |
| Repeaters and comparators | `partial` | [gap regressions](../crates/mc-core/src/redstone/gap_tests.rs), [analog inputs](../crates/mc-core/src/redstone/signal.rs), [redstone_tests.rs](../crates/mc-core/src/redstone/redstone_tests.rs) | Admitted short pulses are extended; cauldrons/composters/cake/hives and through-solid container reads address [VB-04], [VB-05], [VB-06]. Additional analog sources (lecterns/chiseled bookshelves/item frames) and support-loss integration [VB-24] remain separate work. |
| Observers, daylight detectors and sculk sensors | `partial` | [observer pulse audit][blocks-audit]; [daylight regressions](../crates/mc-core/src/redstone/gap_tests.rs); [vibration_tests.rs](../crates/mc-core/src/vibration_tests.rs); [sculk_tests.rs](../crates/mc-core/src/sculk_tests.rs) | Daylight uses local sky, weather darkening and dimension-specific behavior ([VB-07]); observer/sculk fixtures are not whole redstone update-order parity. |
| Pistons/sticky pistons, slime/honey transport, quasi-connectivity | `partial` | [piston_tests.rs](../crates/mc-core/src/redstone/piston_tests.rs); [bounded native entity sweep](../crates/mc-core/src/redstone/piston_motion.rs); [QC oracle pass](QUIRK_PARITY.md#L113-L115) | Admitted moves sweep nearby entities against stationary collision shapes; honey carries grounded bodies. Movement still commits instantaneously: short-pulse spit, transient collision timing and moving-piston dupe window remain absent ([VB-10]; VB-11 timing remains partial). Protected-zone VB-01 has a focused fix. |
| TNT, carpet/rail duping, BUD/update-order and zero-tick contraptions | `partial` | [quirk scenario families](QUIRK_PARITY.md#L81-L94); [known divergences](QUIRK_PARITY.md#L117-L128); [tnt_tests.rs](../crates/mc-core/src/tnt_tests.rs); [native piston/dispenser/dropper QC-BUD regressions](../crates/mc-core/src/redstone/redstone_tests.rs) | Oracle-found TNT BUD duper yields one primed TNT instead of two; carpet/rail dupers share missing moving-piston model. Fluid destroy-list order differs. Native QC regressions do not certify zero-tick/duping or replace the vanilla oracle. |
| Hoppers, droppers, dispensers and crafter | `partial` | [containers_tests.rs](../crates/mc-core/src/redstone/containers_tests.rs); [dispenser block actions](../crates/mc-core/src/redstone/dispenser_blocks.rs); [equipment dispensing](../crates/mc-core/src/redstone/dispenser_equipment.rs); [crafter_tests.rs](../crates/mc-core/src/crafter_tests.rs); [QC note](QUIRK_PARITY.md#L128) | Native coverage includes arrows, TNT, bone meal, filled shulker placement, empty-slot armor equip, waterlogging/source pickup and Nether bucket evaporation. Native dispenser/dropper QC-BUD coverage is not a vanilla oracle scenario; indirect powering still depends on VB-02. Other item actions and blanket transfer/client parity remain unestablished. |
| Rails: ordinary, powered, detector and activator | `partial` | [native rail topology](../crates/mc-core/src/redstone/rails.rs), [gap regressions](../crates/mc-core/src/redstone/gap_tests.rs), [vehicles_tests.rs](../crates/mc-core/src/vehicles_tests.rs) | Neighbor-driven straight/curve/slope selection and reciprocal endpoint power propagation address [VB-08], [VB-09]. Isolated initial placement orientation is a placement integration dependency; exact junction update order and moving-piston rail duping remain outside these fixtures. |
| Water/lava ticks, source creation, buckets and waterlogging | `partial` | [fluid_parity_tests.rs](../crates/mc-core/src/fluid_parity_tests.rs); [fluid owner](../crates/mc-core/src/fluids.rs); [RC-G](#real-client-evidence), source-bucket probe only | Bounded shortest-descent search and joined slab/stair/full-block collision-face checks replace open-side flooding; downward lava conversion preserves waterlogged blocks (VB-12..14). Flow into dry waterloggable containers and exhaustive non-half-grid fluid occlusion are not complete parity claims. |
| Skylight/block light, edits and chunk light delivery | `partial` | [slab face, shape-edit and skylight regressions](../crates/mc-core/src/light_tests.rs); [shared propagation](../crates/mc-world/src/light.rs); [native boundary reconciliation](../crates/mc-core/src/light.rs) | Slab/stair directional faces now gate bake, incremental relight and chunk admission; shape-only edits invalidate both light channels (VB-27). Other shape-occluding families and actual-client visual parity remain unproven. |
| Sand/gravel, concrete powder, falling anvils/stalactites | `partial` | [falling regressions](../crates/mc-core/src/falling_tests.rs); [native falling lifecycle](../crates/mc-core/src/falling.rs) | Empty collision shapes no longer support falling blocks; powder hardens at swept open-water entry; landing hazards deal capped impact damage and anvils degrade (VB-15..17). Helmet impact wear, arbitrary imported HurtEntities parameters and broad actual-client coverage remain gaps. |
| Wheat and sugar-cane production rates (bounded oracle farms) | `works` | [26.1.2 crop/cane oracle results](QUIRK_PARITY.md#L104-L115) | Only named rate fixtures; not all crops, soil, bone meal or farm layouts. |
| Farming, farmland, grass, stems, bamboo, trees and bone meal | `partial` | [farming_tests.rs](../crates/mc-core/src/farming_tests.rs); [farming_environment_tests.rs](../crates/mc-core/src/farming_environment_tests.rs); [plants_tests.rs](../crates/mc-core/src/plants_tests.rs); [composter_tests.rs](../crates/mc-core/src/composter_tests.rs) | Waterlogged/rain irrigation, covered-soil decay, grass/mycelium light/survival, supported fruit, bamboo bone meal and cherry/2×2 pale oak are implemented. Player landing tramples farmland; mob landing remains. Kelp and growing vines have bounded tip growth; ordinary vines currently extend downward only. Mushroom spread/huge growth are bounded simplified shapes. Azalea/mangrove trees, full decorative/fungus/moss bone-meal coverage and exact tree shapes remain incomplete; blocked bone-meal payment differs from vanilla. |

## World generation, dimensions and progression

These rows describe **newly generated** terrain. Imported structure blocks do
not count as native generation. Vanilla-like density/RNG differences alone are
not gaps; missing resources, biomes and structures are.

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Overworld terrain, climate/3D biomes, caves, ores and decoration | `partial` | [terrain.rs](../crates/mc-worldgen/src/terrain.rs); [environment regressions](../crates/mc-worldgen/src/terrain/environment_tests.rs); [feature placement](../crates/mc-worldgen/src/overworld_structures/features.rs); [feature regressions](../crates/mc-worldgen/src/overworld_features_tests.rs) | Deep cavities contain lava at/below -54; badlands have terracotta bands. Dungeons, desert wells, fossils and amethyst geodes use local placed-feature counts/heights/biome membership. Local cave aquifers and defining lush/dripstone cave-biome features remain incomplete ([W06], [W08]); band geometry and feature shapes are simplified ([W07]). |
| Overworld villages and structure lookup | `partial` | [production `/locate` contract](OPERATING.md#announcements-tick-control-and-locate); [saved-start reader](../crates/mc-world/src/structure_starts.rs); [native restart regressions](../crates/mc-world/src/structure_starts_tests.rs); [lookup regressions](../crates/mc-core/src/command_locate_tests.rs); [village generation](VILLAGE_GENERATION.md) | Saved/native/imported starts override plans, including covered empty chunks; queries use metadata without chunk generation and preserve actual underground Y. Eyes and deferred exploration-map searches share the bounded worker. Village and modern-family placement persist start metadata; registry suggestions alone are not generated structures ([W01]). |
| Overworld non-village legacy structures: strongholds, mineshafts, temples, pyramids, igloos, monuments/ruins, outposts, ruined portals, shipwrecks, huts, mansions, treasure | `partial` | [native plans](../crates/mc-worldgen/src/overworld_structures.rs); [classic catalog](../crates/mc-worldgen/src/overworld_structures/catalog.rs); [regressions](../crates/mc-worldgen/src/overworld_structures_tests.rs); [W01] | All classic families now have native starts and interactive contents: stronghold frames/spawner/library, mineshaft rails/spawner, pyramid TNT, temple arrow traps, igloo basements, ocean ruins, monuments, outposts/cages, portals, wrecks, huts, mansions and buried treasure. Template loot remains unopened with its table/seed. Solaris-authored fixed layouts are not full vanilla room assemblers; rotations/processors, stronghold biome relocation, jungle piston puzzle and full monument/mansion variants remain incomplete. Modern jigsaw families have their own row below; eye flight and generated-entity runtime remain separate owners. |
| Nether base terrain | `works` | [volumetric landscape](../crates/mc-worldgen/src/nether/landscape.rs); [cavern/five-biome regressions](../crates/mc-worldgen/src/nether/landscape_tests.rs); [floor/roof/lava/determinism tests](../crates/mc-worldgen/src/nether/tests.rs) | Fixed-lattice stacked caverns, bedrock floor/roof, lava sea and all five biome regions replace the single height field ([W05]); vanilla-like, not exact-seed noise parity. |
| Nether biome-specific landscape, ores and structures (fortresses, bastions, fossils, ruined portals) | `partial` | [landscapes](../crates/mc-worldgen/src/nether/landscape.rs); [imported ore passes](../crates/mc-worldgen/src/nether/ores.rs); [structures](../crates/mc-worldgen/src/nether/structures.rs); [content regressions](../crates/mc-worldgen/src/nether/content_tests.rs) | Five landscapes, quartz/gold/debris, fortress wart/blaze spawners, all four bastion families with inhabitant markers, fossils and ruined portals now generate. Fortress piece diversity/natural-spawn overrides and the complete ruined-portal terrain/aging pass remain incomplete ([W02], [W03], [W05]). |
| Nether/End portals, dimension transfer and portal chunk loading | `partial` | [portals_tests.rs](../crates/mc-core/src/portals_tests.rs); [dimensions_tests.rs](../crates/mc-core/src/dimensions_tests.rs); [world audit portal checks][world-audit] | Empty vehicles use ten-tick cooldown (W12). Passenger assemblies and the 300-tick destination lease remain absent ([W09], [W10]). Native stronghold frames exist; actual-client generated-stronghold→End progression is not established. |
| End central island and obsidian pillars | `partial` | [end.rs](../crates/mc-worldgen/src/end.rs); [native complete pillars/cages/crystals](../crates/mc-core/src/end_fight/pillars.rs); [first-entry pillars/crystals test](../crates/mc-core/src/end_fight/tests.rs#L86) | Central terrain is a Solaris height field; complete landscape/client comparison remains unproven. Native fight initialization owns complete spikes and exit fountain. |
| End outer islands, chorus, cities/ships and naturally obtainable elytra | `partial` | [outer terrain](../crates/mc-worldgen/src/end/outer.rs); [imported city/ship templates and loot](../crates/mc-worldgen/src/end/cities.rs); [natural return gateways](../crates/mc-worldgen/src/end/gateways.rs); [generation regressions](../crates/mc-worldgen/src/end/tests.rs); [chorus growth](../crates/mc-core/src/plants_tests.rs); [reward custody/restart](../crates/mc-core/src/villager/generated_tests.rs) | Resolves absent terrain/rewards in [W04]: imported `end_cities` triangular placement, template cities/ships with treasure and sentries, native framed elytra, highlands return gateways and chorus growth. Assembly topology and island density are independently authored, not complete vanilla recursive-layout/terrain parity; real-client exploration gate remains outstanding. |
| Ender dragon fight, death XP, exit fountain, egg, gateways and crystal respawn | `partial` | [fight tests](../crates/mc-core/src/end_fight/tests.rs); [end_fight.rs](../crates/mc-core/src/end_fight.rs); [gateway landing tests](../crates/mc-core/src/portals_tests.rs) | Native state-machine/restart tests, no complete actual-client dragon fight established. Generated strongholds provide the entrance; post-fight outer exploration and rewards now exist. |
| Natural Overworld→Nether→End survival progression | `partial` | [stronghold generation](../crates/mc-worldgen/src/overworld_structures.rs); [Nether resources and structures](../crates/mc-worldgen/src/nether/structures.rs); [End-city generation](../crates/mc-worldgen/src/end/cities.rs); [W01], [W02], [W03], [W04] | Native stronghold frames, fortress wart/blaze spawners, ancient debris and End-city treasure/elytra now generate. No full fresh-world actual-client survival route through these resources and rewards is certified; imported/admin-supplied content is not evidence for that route. |
| Ancient cities, trail ruins, trial chambers | `partial` | [shared jigsaw implementation](../crates/mc-worldgen/src/village/solver.rs); [modern structure contract](VILLAGE_GENERATION.md#modern-jigsaw-structures) | Cache-driven placement, aliases, template NBT and archaeology processors are wired; live encounters and full underground terrain-adaptation kernels remain unverified. |
| Archaeology | `partial` | [structure generation](../crates/mc-worldgen/src/overworld_structures.rs); [brushing](../crates/mc-core/src/brushing.rs) | Desert pyramids and wells generate brushable sand with seeded loot; warm and cold ocean ruin templates substitute brushable sediment, and trail ruins use their imported processors. Held brushing reveals loot, advances dust and consumes durability. Layouts and ocean-ruin selection are not vanilla-exact; real-client coverage is unproven. |

## Survival loop and game rules

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Hunger, saturation, exhaustion, regeneration and starvation | `partial` | [survival_tests.rs](../crates/mc-core/src/survival_tests.rs); [native parity regressions](../crates/mc-core/src/tick/combat_parity_tests.rs) | VS-03/07 have focused fixes: difficulty-aware exhaustion, Peaceful recovery and starvation enchantment bypass. Harmful foods are covered by [item changes][items-gap]; swimming cost [VS-16] remains separate. |
| XP levels, orbs, Mending and `/experience` | `partial` | [experience_orbs.rs](../crates/mc-core/src/experience_orbs.rs); [survival_tests.rs](../crates/mc-core/src/survival_tests.rs); [death-orb regression](../crates/mc-core/src/tick/combat_parity_tests.rs); [SC-02 fix receipt][commands-fix] | Non-retained deaths award min(level × 7, 100) XP once; keep-inventory/grave custody wins. Furnace bank lifetime is covered by [item changes][items-gap]. Equipped-mob bonus [VC-20] remains separate; max-level overflow is fixed. |
| Death, drops, keep-inventory, death messages and respawn | `partial` | [player.rs death/respawn owner](../crates/mc-core/src/player.rs); [native parity regressions](../crates/mc-core/src/tick/combat_parity_tests.rs); [death notification/team regressions](../crates/mc-core/src/tick/damage_scoreboard_tests.rs); [RC-G](#real-client-evidence) | VS-04/06/14/18 have focused fixes (orbs, cleared fire, configured radius, cleared invalid spawn). `/kill` uses `generic_kill`, distinct from void. Native death components use translation keys; the combat-kill notification is independent of broadcast gamerules and team visibility. Announcements use team filtering and the cross-dimension relay. Missing freeze/collision/cramming/sonic/stalagmite/indirect-magic identities are represented; warden sonic and potion/dragon-cloud producers use their distinct kinds. Broad actual-client death/respawn and detailed fall-assist message parity remain uncertified. |
| Beds/anchors: personal spawn, dimension rules and safe exits | `partial` | [dimensions_tests.rs](../crates/mc-core/src/dimensions_tests.rs); [sleep_tests.rs](../crates/mc-core/src/sleep_tests.rs); [bed exit owner](../crates/mc-core/src/sleep.rs) | Daytime spawn and both-half perimeter/lower-tier exits implemented ([VS-05], [VS-13]); fractional collision-shape exits remain approximate. Invalid bed spawn retention is tracked separately ([VS-18]). |
| Sleeping, multiplayer quorum and skipping night | `works` | [sleep.rs](../crates/mc-core/src/sleep.rs); [sleep_tests.rs](../crates/mc-core/src/sleep_tests.rs) | Native sleeping dimensions, safe wake teleport, quorum and already-clear weather preservation cover [VS-12], [VS-17], [VS-21]. |
| Time, rain/snow and commanded thunder | `partial` | [weather_tests.rs](../crates/mc-core/src/weather_tests.rs); [weather.rs](../crates/mc-core/src/weather.rs); [precipitation owner](../crates/mc-core/src/weather_surface.rs); [farming_tests.rs](../crates/mc-core/src/farming_tests.rs) | Independent retained rain/thunder/clear clocks, exposed rain extinguishing, snow accumulation, weather-independent source-water freezing, block-light melting and precipitation cauldrons implemented. Temperature uses biome-family/altitude approximation; climate-noise and broad melting parity remain unproven. |
| Natural thunderstorms and lightning | `partial` | [weather_tests.rs](../crates/mc-core/src/weather_tests.rs); [lightning owner](../crates/mc-core/src/weather_lightning.rs); [projectile outcomes tests](../crates/mc-core/src/projectiles/payloads_tests.rs) | Native scheduled storm strikes, 128-block rod attraction, rod pulses, fires and entity transformations/damage implemented ([VS-15]). Skeleton-horse traps and copper deoxidation remain absent; no real-client storm gate claimed. |
| Survival/Adventure/Creative/Spectator game modes | `partial` | [movement owner](../crates/mc-core/src/tick/input/movement.rs); [damage admission](../crates/mc-core/src/tick/damage.rs); [mode regressions](../crates/mc-core/src/tick/combat_parity_tests.rs) | VS-01/02 have focused fixes: Creative/Spectator reject ordinary damage before hurt side effects (Void remains lethal); Spectator bypasses terrain collision but retains movement/session checks. Broad Adventure and actual-client mode coverage remain unproven. |
| Peaceful/Easy/Normal/Hard difficulty and gamerules | `partial` | [survival owner/tests](../crates/mc-core/src/survival_tests.rs); [command_rules.rs](../crates/mc-core/src/command_rules.rs); [radius regression](../crates/mc-core/src/tick/combat_parity_tests.rs); [raid difficulty test](../crates/mc-core/src/raids_tests.rs#L251) | VS-03/14 have focused fixes: Peaceful recovery/no starvation and effective respawn radius, including zero and overflow-safe large values. Golem damage [VC-16] remains a separate integration requirement; this is not blanket support for all vanilla gamerules. |
| Statistics, advancements and recipe discovery | `partial` | [statistics_tests.rs](../crates/mc-core/src/tick/statistics_tests.rs); [advancements_tests.rs](../crates/mc-core/src/advancements_tests.rs); [recipe_book_tests.rs](../crates/mc-core/src/inventory/recipe_book_tests.rs); [cultivation acceptance regression](../crates/mc-core/src/farming_environment_tests.rs) | Accepted cultivation emits block-use advancement evidence; rejected footprints do not award it. Furnace-book placement is covered by [item changes][items-gap]. Tests do not cover every vanilla advancement; missing special recipes and natural progression remain ([VI-04], [W01], [W02], [W04]). |

## Saves, packs, administration and plugins

| Player-visible feature area | Status | Evidence | Known gaps / audit finding IDs |
|---|---|---|---|
| Native world save, tick WAL, checkpoints, clean restart and crash recovery | `partial` | [native_checkpoint_crash_tests.rs](../crates/mc-core/src/native_checkpoint_crash_tests.rs); [persistence_crash_tests.rs](../crates/mc-core/src/persistence_crash_tests.rs); [durability contract](OPERATING.md#L565-L595) | Focused crash/custody tests, not universal losslessness. Furnace XP now belongs to the native image ([item closure][items-gap]); unflushed acknowledged tail may roll back consistently. INT-01..05 fixes do not prove every save path. |
| Offline Anvil import/export | `partial` | [conversion contract](OPERATING.md#L565-L595); [roundtrip_tests.rs](../crates/mc-core/src/world_conversion/roundtrip_tests.rs); [world audit conversion checks][world-audit] | Nonempty protochunks served incomplete ([W11]); empty protochunk regeneration can lose opaque data ([W13]); superflat/custom dimensions refused; POI retention is opaque; bossbar export unwired. Inspect each conversion receipt. |
| Live Anvil serving / dual-write | `n/a` | [native-only architecture](ARCHITECTURE.md#L73-L95) | Deliberate storage architecture, not a missing vanilla gameplay mechanic. Use explicit offline interchange; never assume an `.mca` is the live save. |
| Vanilla datapacks: functions, scheduling, custom recipes/loot/worldgen packs | `partial` | [loader/runtime](../crates/mc-core/src/datapacks.rs); [regressions](../crates/mc-core/src/datapacks_tests.rs); [operating limits](OPERATING.md#datapacks-and-command-blocks) | Directory packs, load/tick function tags, bounded context-preserving functions, durable schedule, startup recipe/plain block-loot overrides. Reload replaces functions/tags only; changed recipes/loot require restart. Worldgen/custom dimensions, ZIP packs, macros, arbitrary resource/tag types, non-block loot and full pack management are explicitly unsupported. |
| Server-offered vanilla resource packs and required-pack policy | `partial` | [Configuration pack negotiation](../crates/mc-edge/src/resource_pack.rs); [pack wire/status regressions](../crates/mc-edge/src/resource_pack_tests.rs); [operator configuration](OPERATING.md#server-offered-vanilla-resource-packs) | HTTP(S) URL, SHA1, UUID, JSON prompt and required policy; terminal success required before Play, optional decline allowed. Real-client download/render gate remains unrun; Loader bundles are independent. |
| `server.properties` file compatibility | `partial` | [startup importer](../crates/mc-edge/src/server_properties.rs); [import regressions](../crates/mc-edge/src/server_properties_tests.rs); [mapping and precedence](OPERATING.md#importing-serverproperties) | Supported properties import read-only into the single TOML configuration, explicit TOML leaves win; unsupported keys warn. This is a documented subset, not every vanilla property or automatic world migration. |
| Ops, whitelist, player/IP bans, kick and access-list persistence | `partial` | [operator/access-control contract](OPERATING.md#server-console-and-access-control); [vanilla JSON regression coverage](../crates/mc-edge/src/access_compat_tests.rs); [atomic capacity regressions](../crates/mc-edge/src/server_session_security_tests.rs); [lifecycle regressions](../crates/mc-edge/src/server_leave_tests.rs) | Vanilla JSON persistence, whitelist exemption and fail-closed enforcement; capacity stays reserved during Configuration/pending Join/backpressured Leave, with policy rechecked before Join ([AUTH-01]/[AUTH-02]). Full live access-control matrix remains unproven. |
| RCON and UDP Query | `partial` | [remote service lifecycle](../crates/mc-edge/src/remote_admin.rs); [RCON regressions](../crates/mc-edge/src/rcon_tests.rs); [Query regressions](../crates/mc-edge/src/query_tests.rs); [operating contract](OPERATING.md#rcon-and-udp-query) | Off-by-default authenticated TCP RCON through console and GameSpy4 UDP basic/full stat. External-client interoperability gate remains unrun; asynchronous plugin/backup completion is not a synchronous RCON receipt. Query has bounded requests and a five-second full-stat cache. |
| Native Wasmtime plugins, commands, hooks, permissions and durable KV | `partial` | [versioned deployed WIT contract](PLUGINS.md#L1-L8); [hooks/custody](PLUGINS.md#L51-L109); [plugin host fixes][plugins-fix] | Unsupported hooks (including before-damage) refuse startup; no SIGHUP reload. Direct block zone admission/storage-image size gaps ([Plugin SP-04], [Plugin SP-05]). A pending ABI bump is not tested compatibility; rebuild guests for the deployed WIT version. |
| Bukkit/Spigot/Paper JAR plugin compatibility | `missing` | [component-only deployment](PLUGINS.md#L3-L8), [package format](PLUGINS.md#L27-L32) | Wasmtime components do not execute JVM/Bukkit plugins; no API-compatibility claim for existing JARs. No audit finding ID assigned. |
| Solaris settlements, optional Loader and Distant Horizons | `n/a` | [settlements guide](SETTLEMENTS_GUIDE.md); [Loader](SOLARIS_LOADER.md); **tested with real client — DH Fabric**, recorded in [baseline matrix](https://github.com/kaiserproger/solaris/blob/0a86cfff/docs/COMPATIBILITY.md#distant-horizons); [graphical gate contract](OPERATING.md#L299-L328) | Extensions are not vanilla completion. Baseline records MC 26.1.2 + DH 3.3.3 cold far-LOD/live-geometry pixel pass; no new DH receipt inspected here, NeoForge untested. Material-only parent-LOD swaps remain an upstream limitation. |

## Real-client evidence

- **RC1 — tested with real client, agent-run native Java client.** Retained
  [result.json](../.analysis/codex-logs/FixP0Blocks/ground-after-result.json)
  (`client: java`, `profile: survival`, `status: passed`) and
  [diagnostic report](../.analysis/codex-logs/FixP0Blocks/fixes-report.md#L71-L103),
  run `20261023T044801-survival-p0palge_`, seed 81, server SHA-256
  `daa038d3e5f64eef21b5eb9d355d6946b407cd386c54b0cbf1d9814a4a451423`.
  Thirteen assertions cover joining, ordinary grass mining, authoritative
  removal, dirt pickup, selection, placement and item debit. Correct hand-mining
  rate was measured as 1/18 per tick. This is a worker-build receipt, not a fresh
  integrated-tree replay, full survival completion, combat gate or visual audit
  of every row that cites it.
- **RC-H — tested with real client, historical runtime only.** The retained
  [shared-chest routes](real-client-regression/README.md#L154-L172) name passing
  artifacts `20260622T221125Z-m94-regression-pack` and
  `20260622T232643Z-m94-regression-pack`. Their narrow item visibility/open-menu
  convergence is historical; [the runtime-era warning](real-client-regression/README.md#L8-L13)
  prohibits treating it as current native acceptance.
- **RC-G — gate exists, not a current pass claim.** The approved pack contains
  [water-bucket/solid/sign/recipe probes](real-client-regression/README.md#L117-L153)
  and [death/respawn/restart/two-client routes](real-client-regression/README.md#L173-L204).
  Broad `m94-02` through `m94-07` scenarios are explicitly blocked until their
  missing primitives/evidence exist. A manifest, observed DeathScreen or single
  recipe probe does not turn the broad feature row green.

No new client, server, build or gameplay test was run for this documentation
review. Future receipts must record binary/tree identity, exact scenario,
assertions and artifacts before promoting rows.

## Audit provenance and superseded findings

Audit IDs below link to retained **local, ignored** reports under `.analysis/`.
They resolve in the integrated evidence checkout, but are not shipped by a
normal Git clone. The matrix states the practical gap inline so a reader does
not need private artifacts to understand the status. Those reports' source line
numbers describe their audit tree, not every later refactor. A link is a finding
reference, not a claim that its repro was executed or every proposed fix landed.

| Audit source | Scope / how used |
|---|---|
| [VanCombat][combat-audit] | VC-01..20: combat and mob parity gaps; bounded positive source checks. |
| [VanItems][items-audit] | VI-01..14: item/station/container gaps; **VI-01 is superseded** by timed-mining fixes and RC1. |
| [VanBlocks][blocks-audit] | VB-01..30: blocks/redstone/fluids/light; **VB-01 is superseded** by protected-zone piston admission. |
| [VanSurvival][survival-audit] | VS-01..21: survival, modes, weather and respawn gaps. |
| [VanWorld][world-audit] | W01..13: fresh generation, portals and Anvil boundaries. |
| [SecAuth][auth-audit] | AUTH-01/02/03/06/07/08/09 have native/edge lifecycle, private-custody, committed-LOD and locked-plate/dimension fences in [GapProtocol][gap-protocol]. Sibling password-plugin AUTH-04/05 remains open; no new plugin-client acceptance claim. Online Mojang identity verification itself was checked fail-closed. |
| [SecCommands][commands-audit] | SC-06 has native book editing/signing admission and retained component codecs; actual-client coverage remains pending. SC-03/05 have bounded signed-command decoding and separate command length admission in [GapProtocol][gap-protocol]. SC-01/02/04 have [focused fixes][commands-fix] (spam, XP overflow, mute command admission). Secure-chat signature verification remains unsupported. |
| [SecProtocol][protocol-audit] | Protocol SP-03 remains an audit gap. SP-04/05 have matching keepalive/deadline fixes in [GapProtocol][gap-protocol]. SP-01/02 have [focused fixes][protocol-fix]; listener policy is documented in [OPERATING](OPERATING.md). |
| [SecPlugins][plugins-audit] | Plugin SP-04/05 remain audit gaps. SP-01/02/03 have [focused host fixes][plugins-fix] for config escape and permission publication/retirement. IDs overlap SecProtocol; prefixes here disambiguate them. |
| [IntegrityDupes][integrity-audit] | INT-01..05 have [focused fixes][integrity-fix] and current [custody rules](ARCHITECTURE.md#L81-L88); do not list the old chest/allay/round-trip/cursor/dead-pickup exploits as unfixed. |

The original [audit summary](../.analysis/codex-logs/Audit-SUMMARY.md) is a
historical inventory, **not a current open-defect count**. For fixed mining and
piston admission, see [mining corrections/RC1][mining-fix],
[mining regression tests](../crates/mc-core/src/tick/mining_tests.rs), and
[piston protected-footprint regression:48](../crates/mc-core/src/redstone/piston_tests.rs#L48).
For families with no assigned finding, `partial` explicitly records the evidence
limit instead of inventing an audit ID or claiming full compatibility.

[combat-audit]: ../.analysis/codex-logs/Audit-VanCombat/report.md
[items-audit]: ../.analysis/codex-logs/Audit-VanItems/report.md
[items-gap]: ../.analysis/codex-logs/Gap-GapItems/report.md
[blocks-audit]: ../.analysis/codex-logs/Audit-VanBlocks/report.md
[survival-audit]: ../.analysis/codex-logs/Audit-VanSurvival/report.md
[world-audit]: ../.analysis/codex-logs/Audit-VanWorld/report.md
[auth-audit]: ../.analysis/codex-logs/Audit-SecAuth/report.md
[commands-audit]: ../.analysis/codex-logs/Audit-SecCommands/report.md
[protocol-audit]: ../.analysis/codex-logs/Audit-SecProtocol/report.md
[plugins-audit]: ../.analysis/codex-logs/Audit-SecPlugins/report.md
[integrity-audit]: ../.analysis/codex-logs/Audit-IntegrityDupes/report.md
[commands-fix]: ../.analysis/codex-logs/FixP0Commands/report.md
[protocol-fix]: ../.analysis/codex-logs/FixP0Protocol/report.md
[plugins-fix]: ../.analysis/codex-logs/FixP0Plugins/report.md
[integrity-fix]: ../.analysis/codex-logs/FixP0Dupes/report.md
[mining-fix]: ../.analysis/codex-logs/FixP0Blocks/fixes-report.md
[gap-protocol]: ../.analysis/codex-logs/Gap-GapProtocol/report.md
[AUTH-01]: ../.analysis/codex-logs/Audit-SecAuth/report.md#L11
[AUTH-02]: ../.analysis/codex-logs/Audit-SecAuth/report.md#L27
[AUTH-03]: ../.analysis/codex-logs/Audit-SecAuth/report.md#L42
[Plugin SP-04]: ../.analysis/codex-logs/Audit-SecPlugins/report.md#L70
[Plugin SP-05]: ../.analysis/codex-logs/Audit-SecPlugins/report.md#L89
[Protocol SP-04]: ../.analysis/codex-logs/Audit-SecProtocol/report.md#L70
[Protocol SP-05]: ../.analysis/codex-logs/Audit-SecProtocol/report.md#L87
[SC-03]: ../.analysis/codex-logs/Audit-SecCommands/report.md#L27
[SC-05]: ../.analysis/codex-logs/Audit-SecCommands/report.md#L41
[SC-06]: ../.analysis/codex-logs/Audit-SecCommands/report.md#L48
[VB-02]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L18
[VB-03]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L24
[VB-04]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L30
[VB-05]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L36
[VB-06]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L42
[VB-07]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L48
[VB-08]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L54
[VB-09]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L60
[VB-10]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L66
[VB-11]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L72
[VB-12]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L78
[VB-13]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L84
[VB-14]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L90
[VB-15]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L96
[VB-16]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L102
[VB-17]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L108
[VB-18]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L114
[VB-19]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L120
[VB-20]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L126
[VB-21]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L132
[VB-22]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L138
[VB-23]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L144
[VB-24]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L150
[VB-25]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L156
[VB-26]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L162
[VB-27]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L168
[VB-28]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L174
[VB-29]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L180
[VB-30]: ../.analysis/codex-logs/Audit-VanBlocks/report.md#L188
[VC-01]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L14
[VC-02]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L24
[VC-03]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L34
[VC-04]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L44
[VC-05]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L54
[VC-06]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L64
[VC-07]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L74
[VC-08]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L84
[VC-09]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L94
[VC-10]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L104
[VC-11]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L114
[VC-12]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L124
[VC-13]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L134
[VC-14]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L144
[VC-15]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L154
[VC-16]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L164
[VC-17]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L174
[VC-18]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L186
[VC-19]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L196
[VC-20]: ../.analysis/codex-logs/Audit-VanCombat/report.md#L206
[VI-02]: ../.analysis/codex-logs/Audit-VanItems/report.md#L20
[VI-03]: ../.analysis/codex-logs/Audit-VanItems/report.md#L27
[VI-04]: ../.analysis/codex-logs/Audit-VanItems/report.md#L34
[VI-05]: ../.analysis/codex-logs/Audit-VanItems/report.md#L41
[VI-06]: ../.analysis/codex-logs/Audit-VanItems/report.md#L48
[VI-07]: ../.analysis/codex-logs/Audit-VanItems/report.md#L55
[VI-08]: ../.analysis/codex-logs/Audit-VanItems/report.md#L62
[VI-09]: ../.analysis/codex-logs/Audit-VanItems/report.md#L69
[VI-10]: ../.analysis/codex-logs/Audit-VanItems/report.md#L76
[VI-11]: ../.analysis/codex-logs/Audit-VanItems/report.md#L83
[VI-12]: ../.analysis/codex-logs/Audit-VanItems/report.md#L90
[VI-13]: ../.analysis/codex-logs/Audit-VanItems/report.md#L97
[VI-14]: ../.analysis/codex-logs/Audit-VanItems/report.md#L104
[VS-01]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L15
[VS-02]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L22
[VS-03]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L29
[VS-04]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L36
[VS-05]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L43
[VS-06]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L50
[VS-07]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L57
[VS-08]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L64
[VS-09]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L71
[VS-10]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L78
[VS-11]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L85
[VS-12]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L92
[VS-13]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L99
[VS-14]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L106
[VS-15]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L113
[VS-16]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L120
[VS-17]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L129
[VS-18]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L136
[VS-19]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L143
[VS-20]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L150
[VS-21]: ../.analysis/codex-logs/Audit-VanSurvival/report.md#L157
[W01]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L13
[W02]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L21
[W03]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L29
[W04]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L37
[W05]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L45
[W06]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L53
[W07]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L61
[W08]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L69
[W09]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L77
[W10]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L85
[W11]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L93
[W12]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L101
[W13]: ../.analysis/codex-logs/Audit-VanWorld/report.md#L109
