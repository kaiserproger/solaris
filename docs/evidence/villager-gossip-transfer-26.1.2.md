# Villager gossip transfer oracle — Java 26.1.2

This note records the bounded facts used by the Solaris gossip-transfer checkpoint. The decompiled classes are local analysis inputs and are not distributed by the repository; their exact workspace-relative paths and SHA-256 hashes are recorded so an owner with the analysis bundle can reproduce the inspection.

## Source fingerprints

| Class | Local path | SHA-256 |
| --- | --- | --- |
| `GossipContainer` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/ai/gossip/GossipContainer.java` | `1850d3286947e0684ac4de4e61d1cb8b049fd926616827ab68cfee0c15942a2a` |
| `GossipType` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/ai/gossip/GossipType.java` | `a3bde600b3341e1438af6746a83df00d423c970418f38ec5a6fd1f33365128d1` |
| `TradeWithVillager` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/ai/behavior/TradeWithVillager.java` | `bf2fa952ec62ccf881eb8af1f39cf756fbce778e506b3a1e6492003e1ad6c6a1` |
| `VillagerGoalPackages` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/ai/behavior/VillagerGoalPackages.java` | `447b0c9e5403446c8dd1b6bcb32ca47580fd40f2e3218acffa3d35d4ba31ea99` |
| `Villager` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/npc/villager/Villager.java` | `d25a1785107d7a35f3498090b516378a3b32ec39e5c597c596ac7cbc781758f6` |

## Confirmed transfer facts

- `TradeWithVillager` runs from the vanilla `Idle` and `Meet` behavior packages.
- When the interaction target is a villager and `distanceToSqr(target) <= 5`, the initiator calls `initiator.gossip(target)`.
- Transfer direction is target/source into initiator/receiver.
- Both villagers must pass the same `lastGossipTime` fence: time moved backwards, or at least 1,200 ticks elapsed. A successful attempt writes the same timestamp to both villagers.
- `lastGossipTime` and the interaction target are runtime state; only the gossip container and daily-decay timestamp are saved.
- `GossipContainer` makes at most ten weighted random draws, weighting each entry by the absolute weighted reputation, and deduplicates repeated selections.
- A selected entry loses its type's `decayPerTransfer`, is discarded below the stored-value floor `2`, and merges into an existing entry with `max(old,new)`.
- For the currently implemented Solaris types, both `TRADING` and `MINOR_NEGATIVE` have transfer decay `20`. Their weights have absolute magnitude `1`, so their selection weight is their stored value.

## Deliberate RNG boundary

Solaris ports Java's 48-bit legacy `nextInt(bound)` algorithm for each transfer selection. It does **not** yet reproduce the complete per-entity vanilla `RandomSource` stream, because Solaris does not own the full sequence of every random draw consumed by a Java villager. The initial seed is deterministically derived from receiver UUID, source UUID, and simulation tick. Therefore:

- selection bounds, weighting, draw count and Java `nextInt` behavior are covered;
- the exact identity of entries selected for an arbitrary vanilla world is not claimed bit-for-bit;
- replacing the Solaris seed source with a complete entity `RandomSource` stream remains a separate parity improvement, not a hidden claim of this checkpoint.
