# M68 Container Click Coverage

M68.c cleanup started with the existing coverage before deduplicating click
classification. This matrix records current behavior coverage; it is not a new
parity claim.

| Menu | Pickup | Quick move | Swap | Throw | Persistence / wire coverage |
|---|---|---|---|---|---|
| Player inventory | unit coverage for merge/cursor helpers; harness covers armor equip through inventory click | unit coverage for armor shift-equip and crafting result helpers | unit coverage for hotbar/offhand swap mapping | unit coverage for one/full stack throw helper | harness covers armor slot mutation and survival drops/pickup |
| 2x2 inventory crafting | unit coverage for result consume/remainders via inventory crafting tests | unit coverage through crafting result quick-move helpers | covered by shared player swap mapping | covered by shared throw helper | unit coverage only |
| Crafting table | harness covers placing inputs and taking shaped/shapeless results | harness covers recipe-driven movement indirectly through result handling | M69.b harness covers open-window swap | M69.b harness covers open-window throw | harness `crafting_table_container_crafts_shapeless_and_shaped_results` |
| Furnace/smoker/blast furnace | harness covers input/fuel/result slot mutation during smelting | covered by furnace click core tests through existing unit paths | M71.c unit coverage exercises furnace-window swap adapter | M71.c unit coverage exercises furnace-window throw adapter | harness `survival_furnace_container_smelts_input_with_fuel`; M71.c focused unit coverage |
| Chest/double chest | harness covers taking from a double chest half | harness coverage indirect via storage mutation paths | M71.c unit coverage exercises chest-window swap adapter | M71.c unit coverage exercises chest-window throw adapter | harness `survival_double_chest_opens_combined_storage_and_mutates_second_half`; M71.c focused unit coverage |

## Cleanup Notes

- M68.c centralizes packet click classification into one helper so player,
  crafting, furnace, and chest handlers share slot/button validation.
- Slot mutation remains menu-specific in this slice because result slots, furnace
  validation, chest persistence, and player armor slots still have different
  side effects.
- M71.c added focused unit assertions for swap/throw inside furnace and chest
  windows. Crafting-table swap/throw has harness coverage from M69.b.
