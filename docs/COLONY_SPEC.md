# Colony Plugin Spec (draft for owner approval)

Scope: server-only Luau plugin `colony-villager-scaffold`, API `0.6.0`, vanilla
26.1.2 client. Colony identity, roles, orders, and durable intent stay in
Luau/plugin storage. Rust exposes only generic villager binding and
movement/idle goal primitives (`bind_nearest_villager`, `move_villager_to`,
`set_villager_idle`; goals `idle` / `follow_position`). Nothing in this spec
adds Rust primitives; gaps that need them are listed as open decisions.

## Done (committed)

- Recruit: `/colony recruit [role]` and right-click villager
  (`player.entity_interacted`); one active villager per player; durable record
  `{status, role, order, generation}` via storage CAS.
- Roles `worker/guard/farmer`: stored, changeable (`/colony role`), validated
  against config. **Behaviorally inert today** — this spec gives them meaning.
- Orders `home/hold/follow`: `home`/`follow` map to `follow_position` (colony
  home anchor / live player position via block/interact refresh), `hold` to
  `idle`. Binding refresh with single retry; transient refresh failures keep
  the active lease.

## Done (this checkpoint, wire-tested)

- Role defaults (D1/D2): guard recruits to `follow`, worker/farmer to `home`;
  `/colony role <role>` switches to the role default and re-issues the goal.
  Gate: `shipped_colony_scaffold_role_defaults_and_role_switch_over_wire`.
- Growth (D3): `limits.max_active_members` (default 3, hard cap 8);
  `member:<uuid>[:<slot>]` keys with lazy slot-0 migration; per-member orders;
  over-limit recruit rejected. Gate: `shipped_colony_scaffold_growth_*`
  (recruit/limit and order/dismiss/rerecruit halves, split to respect the
  ingress 8-burst/2-per-second command budget).
- Full release (D6): `/colony dismiss [n]` idles the villager through a
  correlated `solaris.release_villager_binding` lease release (new API
  `0.6.0`, capability `villagers`, result `villager.release_result`), then
  persists a `released` tombstone via storage CAS so reclaimed request ids
  stay unique across dismiss/recruit cycles and restarts. Removed-villager
  recovery rebinds a live villager per member and preserves generations.
- Typing: the scaffold is fully annotated Luau `--!strict` (domain
  record/pending/config types; host-boundary `event: any` retained);
  the discovery type gate enforces it on load.
- Packaging: the bundled server embeds `config.toml` (the strict chunk
  requires it); guard `bundled_colony_scaffold_starts_with_its_config`
  starts the bundled host to `loaded_plugins()==1`.
- Live gate: `m94-colony-villager-live-qa` PASSED on the real 26.1.2
  graphical client (recruit/status/role/order/dismiss/re-recruit to
  generation 5, measured follow approach, screenshot, 5 adversarial
  checks; one P3 single 267 ms physics tick, no correction/disconnect).

## Proposed: professions (role = behavior preset, order still wins)

Role sets the *default order at recruit* and constrains nothing else.
Roles and orders are config-driven (`config.toml`); the tables below pin the
shipped default config values. Explicit `/colony order` always overrides; `/colony role` re-applies the
role's default order only when the player passes no order (new optional second
argument, undecided — see D2).

| Role   | Recruit default order | Intended fantasy            |
| ------ | --------------------- | --------------------------- |
| worker | `home`                | stays at the colony anchor  |
| guard  | `follow`              | shadows its player          |
| farmer | `home`                | stays at the colony anchor  |

Worker/farmer share behavior until Rust gains work primitives; the labels
persist in the record so future behavior can diverge without migration.

## Proposed: tasks (order vocabulary, all on existing primitives)

Keep `home/hold/follow`. No new order words until a Rust primitive justifies
one (patrol/guard-post/attack need movement loops or combat targeting that do
not exist; damage_entity exists but there is no targeting/aggro input).

## Proposed: growth (colony grows member by member)

- Raise the per-player member limit from 1 to a new `limits.max_active_members`
  (default 3, hard cap 8). It is orthogonal to the existing
  `limits.max_active_players` (how many players may hold colonies): members
  live under storage keys `member:<uuid>:<slot>`, well within the 128-byte
  key limit. One binding/goal pipeline per member (reuse the pending tables
  keyed by member, not player).
- `/colony status` lists all members; `/colony order` gains an optional member
  index (default: all members).
- Existing single-member records migrate lazily (slot 0); no journal rewrite.

## Acceptance per slice (wire gates, no manual client)

1. Role defaults: recruit guard → `follow` goal issued; recruit worker/farmer
   → `home` goal; stored roles correct; `/colony role` + explicit order honored.
2. Growth: recruit up to the limit; over-limit recruit rejected with a message;
   per-member orders apply to the right lease; removed-villager recovery works
   per member; second save preserves the full member set.

## Non-goals (need Rust/AI work, not this spec)

Combat/aggro, work animations, item pickup/farming, villager-to-villager
interaction, cross-player colonies, Loader UI.

## Owner decisions (answered 2026-09-04)

- D1: APPROVED — guard→follow, worker/farmer→home.
- D2: APPROVED — `/colony role <role>` also switches the order to the role
  default and re-issues the villager goal.
- D3: spec default stands (max_active_members 3, hard cap 8) — not overridden.
- D4: APPROVED — role defaults first, growth second.
- D5: spec default stands (recruit-order index) — not overridden.
- D6: APPROVED — full release (free slot, unbind, villager to idle).
