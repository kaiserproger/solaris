# Alpha-3 standard plugin pack (P2 items 1–5)

Date: 2026-08-30

## What shipped

The five first-party API 0.6 server-only packages under
`examples/plugins/` (authored on the carried alpha-3 worktree, first
committed in `5b8b54c7`):

- `solaris-permissions` — durable groups and permission-node catalog.
- `solaris-essentials` — homes, spawn/warps, back, TPA, private messages.
- `solaris-economy` — single virtual-money authority with idempotent
  transfer tokens and a bounded ledger.
- `solaris-towns` — small towns, invitations, roles, leader-protected chunk
  claims.
- `solaris-audit` — bounded committed-action history with filtered lookup.

Deployment contract: external-directory packages copied into
`[plugins].directory`; the deliberate no-bundled-wiring decision and the
P2 item 5 scope decision (intentional omissions) are recorded in
`examples/plugins/standard-pack/README.md`; operator docs in
`docs/PLUGINS.md` ("Standard plugin pack").

## Verification

`cargo test -p mc-test-harness --test plugin_standard_pack`: 6/6 passed.

- strict load of all five plugins with per-plugin startup contracts;
- permissions promote a player through a storage CAS roundtrip;
- essentials saves and revisits a home (player-teleport command);
- economy pays once per transfer token (ledger + token window commit,
  replay rejected idempotently);
- towns claim a leader-protected chunk across a host restart;
- audit records committed actions and serves bounded filtered lookups.

One assertion was corrected in the test itself during review: the economy
ledger encoding writes accounts in the plugin's table order, so the test
accepts either account ordering while asserting the token window and
balances exactly.

## Boundaries

No mc-server wiring change (documented decision). No new runtime
dependencies. The remaining demonstration examples are unchanged.
