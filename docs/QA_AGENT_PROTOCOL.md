# Solaris break-it QA protocol

A gate that walks the author's happy path proves only that path. During the
2026-10-31 RC2/RC3 release work, broader player-path checks exposed three gaps
that the earlier checks had not covered:

- fresh-install singleplayer could not use the pinned content bundle: it lacked
  a file the new server required;
- the 26.3 Loader adapters crashed at startup on stale mixin targets;
- a NeoForge Loader client could not send its reply on an undeclared channel.

These failures required different checks, not more repetitions of the same
happy path. The protocol below makes those attacks part of acceptance; a green
receipt remains evidence only for the exact path and artifact it exercised.

## Roles

- **Builder** implements the change and its focused/failing-first tests and gates.
- **Breaker** is a separate agent whose only goal is a reproducible failure. It
  gets the change, the claims (commit message, release notes, ADR text) and this
  protocol, not the builder's test plan. It does not edit production code.
- **Main** routes findings back to the builder and decides on release. A change
  that touches player-visible behaviour is not pushed to `main`, and a release
  candidate is not tagged, until a breaker has reported on it.

Use a normal player's/admin's expectations and vanilla 26.3 evidence
(minecraft.wiki, `javap`) where vanilla applies. For Solaris-specific features,
the owner's accepted behaviour is the contract; implementation and green tests
are not the oracle. The earlier protocol already required bounded adversarial
exploration: this revision strengthens its execution and coverage evidence.
"Found nothing" is valid only with the attacks actually tried.

## Attack matrix

For every change, pick each class below that the change can plausibly affect, and
run at least one concrete attack per class. Write down why each skipped class
does not apply.

| Class | Attack | Example miss it catches |
|---|---|---|
| Zero state | Fresh `HOME`/data root and empty caches: frozen staging package before the tag, downloaded release assets afterward, never the dev tree | content bundle missing `en_us.json` |
| Previous release | Start on the previous release's world, content cache, config and client data | RC1 cache and worldgen revision refused |
| Every advertised variant | Every platform, loader, OS and client the notes promise: Fabric/NeoForge/Forge, native/vanilla Java, Linux/Windows/macOS, 26.3 and 26.1.2 | Loader mixins, NeoForge channel declaration |
| Every branch | Each outcome the change introduces or alters (started/queued/refused, funded/unfunded, owner/non-owner) at least once on a real client | queue gate only saw an immediate `Started` |
| Interruption | Restart, `kill -9`, disconnect or relog in the middle of the action; resume after | pending order lost on restart |
| Resources and limits | Empty and full inventory/storage, zero/max values, max level, radius edge, chunk/simulation boundary | builder idle in an inactive chunk |
| Actors | Non-owner, operator, second player at once, player in another dimension | owner notification across dimensions |
| Hostile input | Malformed, out-of-order, repeated, oversized or unicode input; commands with wrong arguments | — |
| Time and variance | Different seed, a second run, slower host (`taskset` 2 CPUs), long session | autonomy gate passed on one run and failed on the next |
| Claims | Every sentence in the commit message, release notes or README is backed by a receipt; anything else is reworded | "vanilla-exact", "caches keep working" |

## Gate hygiene

A gate counts as evidence only when all of these hold:

1. **Negative control.** A new or changed behavioural gate fails without the fix
   or feature, at the expected assertion. Record that receipt. Missing fixtures,
   compilation errors and client-startup failures are not a negative control
   for unrelated gameplay assertions.
2. **No pre-existing pass.** Each assertion observes something the action
   produced: a fresh event, a new identity, a changed counter. State that could
   already exist before the action (an earlier project of the same type, an old
   chat line) does not count.
3. **Branch coverage.** The real client exercises every player-visible outcome
   the change introduces or alters. Other outcomes retain the appropriate
   protocol/native checks. Record unexercised branches explicitly; unit tests
   are not a substitute for claimed real-client coverage.
4. **Variance.** A gate with timing-, seed- or concurrency-dependent behaviour
   reports a pass rate over at least three runs or seeds. One pass is not
   reliability, and one failure is a finding until it is explained.
5. **Artifact identity.** Before the tag, record hashes of a frozen package built
   through the shipping packaging path. After publication, download the actual
   assets and check their hashes, contents and clean install. Neither a local
   binary beside dev data nor a successful CI build proves shipped installation.

## Release-candidate breaker pass

### Before the tag: frozen candidate

- Stage the package through the release's existing packaging path. Record the
  source revision, binary/package hashes, content pins and plugin pack identity.
  Freeze those inputs for the run; do not require a published asset that does not
  exist yet.
- On the staged package, run the applicable clean-install, previous-release
  upgrade and platform/Loader attacks from the matrix. Include the real launcher
  and singleplayer for a native-client release.
- Run `loader-live` on advertised Loader platforms and `title-connect` on the
  native client when the candidate affects them. Audit release claims against
  receipts; a missing platform or environment is blocked coverage, not a pass.

### After publication, before announcement: distribution smoke

- Download the real release assets, verify checksums and inspect package contents
  and metadata. Record the downloaded artifact identities.
- Exercise clean install/start/join using those files and the public manifest,
  without dev-data overrides. Verify that shipped content and plugin pins resolve.
- If CI-produced bytes differ from the staged package, record that identity
  separately. Re-run affected functional attacks only when the code, dependencies,
  configuration or package/content identity changed meaningfully; do not repeat
  unchanged full suites just because publication happened.
- Check release-page links and upgrade instructions. A failed distribution smoke
  blocks announcement even if the pre-tag functional gates passed.

## Breaker procedure

1. Read `AGENTS.md`, this protocol and the change: diff, claims and changed gates.
2. Build the attack list from the matrix: concrete steps per applicable class and
   a reason for each skipped class. Spend most of the time on the classes closest
   to the change.
3. Run the attacks on real binaries (release server for real-client gates) with
   the canonical harness. Use a focused runner only when no existing runner
   expresses the attack. Before the first destructive/security or migration
   attack, make disposable local copies, including previous-release worlds;
   never use the owner's originals or live server. Use private ports, data roots
   and Xvfb per breaker; kill only processes you started.
4. For a failure, retain the first failing receipt and reduce it to a repro.
   Re-run once only when it adds evidence, on disposable local state. Do not
   recreate corruption or repeat a destructive/security action on live data.
   Report both attempts (`repro n/m`) or why confirmation was unsafe/unavailable.
5. Check gate hygiene on every gate the change added or modified.
6. Also perform a bounded independent exploratory pass: choose an attack not
   copied from the builder's script or exhausted by the prescribed checklist.
   Record the hypothesis, action and result even when it holds.
7. At assignment close, report attempted and planned-but-unexercised attacks and
   branches. Time-box expiry is not coverage: if a required attack remains
   unexercised, return `blocked`, not `pass`, with its prerequisite and next step.

Findings are fixed by the builder with a failing-first test, and a confirmed
consumer-visible failure is added to the nearest gate or scenario. The breaker
does not fix the implementation it is judging. After a relevant fix, re-run the
failed attack; a second independent review is not required for the same checkpoint.
P0/P1 findings or blocked required coverage prevent acceptance; P2/P3 findings stay
visible with an explicit disposition rather than silently disappearing.

## Severity

- **P0**: crash, disconnect, data loss/corruption, dupe, security, or the player
  cannot do the advertised thing at all.
- **P1**: needs a non-obvious workaround, or the advertised claim is false.
- **P2**: wrong against vanilla or the docs, with an obvious workaround.
- **P3**: cosmetic.

## Environment-only exceptions

An OpenAL/audio error under Xvfb is environment noise only when the client still
enters and stays in Play. GLFW/display failures, server disconnects, correction
storms, ghost state, missing publications and bridge timeouts are findings.

## Result contract

```yaml
verdict: pass | changes | blocked
target: <commit/tag/asset hashes attacked>
attacks:
  - class: <matrix class>
    steps: <what was done>
    result: held | broke
skipped_classes: [{class, reason}] # genuinely inapplicable only
unexercised_required: [{attack, branch, reason, next_step}]
exploratory_attacks: [{hypothesis, action, result, evidence}]
gate_hygiene: [{gate, negative_control, pre_existing_pass, branches, runs}]
findings:
  - severity: P0 | P1 | P2 | P3
    title: <concrete issue>
    repro: <command or steps>
    repro_rate: <n/m>
    expected: <vanilla/doc/claim>
    actual: <observed>
    evidence: [<server log, client log, result.json, screenshots>]
```

Final line: `QA_RESULT=PASS`, `QA_RESULT=CHANGES`, or `QA_RESULT=BLOCKED`.
