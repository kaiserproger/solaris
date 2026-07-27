# Villager positive gossip oracle — Java 26.1.2

This note records the bounded Java facts used by the Solaris positive-gossip checkpoint. Decompiled classes are local analysis inputs and are not distributed by the repository. Paths and SHA-256 fingerprints let an owner with the analysis bundle reproduce the inspection.

## Source fingerprints

| Class | Local path | SHA-256 |
| --- | --- | --- |
| `GossipType` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/ai/gossip/GossipType.java` | `a3bde600b3341e1438af6746a83df00d423c970418f38ec5a6fd1f33365128d1` |
| `ReputationEventType` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/ai/village/ReputationEventType.java` | `0792e8d75c215ff0412737a2b31019a5d0b12a56ec6da662942f40e938432b87` |
| `Villager` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/npc/villager/Villager.java` | `d25a1785107d7a35f3498090b516378a3b32ec39e5c597c596ac7cbc781758f6` |
| `ZombieVillager` | `.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/monster/zombie/ZombieVillager.java` | `0cc97883a080f6cae8322c27988c7e3704f9b96147b9f1b06c208ca7bc341e8d` |

## Confirmed facts

- `MINOR_POSITIVE` has weight `+1`, maximum stored value `25`, daily decay `1`, and transfer decay `5`.
- `MAJOR_POSITIVE` has weight `+5`, maximum stored value `20`, daily decay `0`, and transfer decay `20`.
- `ZOMBIE_VILLAGER_CURED` adds `MAJOR_POSITIVE +20` and `MINOR_POSITIVE +25` for the curing player.
- The event is emitted only after zombie-villager conversion completes and the retained conversion starter resolves to a server player.
- The shared stored-value floor is `2`. Therefore a maximum `MAJOR_POSITIVE` entry loses all `20` points when selected for transfer and is discarded; everlasting cure memory does not propagate. A maximum `MINOR_POSITIVE` entry transfers as `20`.
- Both positive types participate in total weighted reputation and the existing weighted gossip-selection pool.

## Solaris boundary

Solaris now persists, validates, prices, decays, and transfers both positive gossip types and exposes the exact atomic `ZombieVillagerCured` state event. Solaris does not yet implement zombie-villager curing/conversion gameplay, so no production runtime path emits that event in this checkpoint. Conversion timing, ingredients/effects, entity replacement, advancement, sound, and real-client evidence remain separate work.
