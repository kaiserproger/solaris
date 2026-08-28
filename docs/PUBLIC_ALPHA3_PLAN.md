# Solaris v0.0.3-alpha.1 Carry-Over Plan

Date: 2026-08-28
Target: `v0.0.3-alpha.1`

`v0.0.2-alpha.1` is closed with every automated release gate green. The only
release-2 acceptance row not personally dispositioned by the owner was the
subjective terrain/playability judgement for fresh `tellus_like` seed `712816`.
The owner explicitly deferred that judgement to this third alpha instead of
blocking the second release or treating the judgement as a PASS.

## First checkpoint — owner worldgen disposition

- [ ] Review the prepared seed-`712816` material in
  [`evidence/phase4-seed-712816-owner-disposition-package-2026-08-28.md`](evidence/phase4-seed-712816-owner-disposition-package-2026-08-28.md), including the
  canonical 12-view contact sheet and the 2048x2048 height/biome/vegetation
  mosaics.
- [ ] Owner disposition is explicit: `ACCEPT`, or `REJECT` with one concrete
  terrain/playability defect.
- [ ] On `REJECT`, make that concrete defect the first focused worldgen checkpoint;
  do not reopen already-passing traversal, persistence, natural-spawn, throughput,
  or release-2 gates without a regression.
- [ ] On `ACCEPT`, record the owner disposition and continue with the next measured
  common-play/plugin/performance priority rather than manufacturing more worldgen
  work.

The automated seed evidence remains valid carry-over context, not a substitute
for the owner judgement.
