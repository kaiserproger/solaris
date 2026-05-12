# Vanilla data — extracted from the Minecraft Java Edition server jar

This directory is the local-only sidecar Solaris reads at runtime to
populate the registries the protocol mandates we send to connecting
clients.

**Nothing under `data/vanilla/data/`, `data/vanilla/reports/`, or
`data/vanilla/version.json` is committed to git.** Those paths are
`.gitignore`d. Only this README (and any future schema/tooling docs
you put alongside it) lives in version control.

Files are populated locally by running
[`tools/extract-vanilla-data.sh`](../../tools/extract-vanilla-data.sh)
against the Mojang server jar at `.analysis/server.jar`. The contents
are Mojang's, not Solaris'.

See
[`docs/decisions/0001-vanilla-data-as-runtime-input.md`](../../docs/decisions/0001-vanilla-data-as-runtime-input.md)
for the reasoning. PROJECT_SPEC §8.1 has been updated accordingly.

## Provenance

- Source: Minecraft Java Edition server `26.1.2` (protocol version
  `775`), released 2026-04-09.
- Build hash and exact byte hashes of each file are reproducible from
  the same server.jar; we do not check those hashes in here, because
  Mojang's official server jar is itself the canonical source of
  truth — if our extraction diverges from it, our extraction is
  wrong.

## Legal note

Mojang's data files are **not** licensed under MIT/Apache-2.0 (those
licenses cover *Solaris' own source code* only). Solaris does not
redistribute them: the operator of a Solaris server has to supply a
`server.jar` they obtained legitimately and run the extraction script
locally. Players connecting to that server need a legitimate Minecraft
license, just as they would for any vanilla or third-party server.

## Reproducing this extraction

```sh
tools/extract-vanilla-data.sh
```

The script pulls a deterministic subset from `.analysis/server.jar`:

- `data/minecraft/<registry>/` — the top-level registry directories
  the Configuration state's `RegistryData` packet references.
- `data/minecraft/worldgen/biome/` — biome JSONs.
- `data/minecraft/tags/` — the entire tags tree (used by the
  `UpdateTags` packet from M2 onward).
- `reports/blocks.json`, `reports/registries.json`, `reports/packets.json`
  — produced by running the server's own `--reports` data generator.
  `blocks.json` is the canonical block-state-id mapping `mc-world`
  loads at startup (M2.b); the other two are kept as cross-check
  oracles for the protocol layer.

Everything else (recipes, loot tables, advancements, full worldgen,
structures) is left out by default — add to the `REGISTRIES` list in
the script when a later milestone needs it.

Generating reports invokes the bundled server's own data generator,
which requires Java matching `version.json`'s `java_version` field
(25 for 26.1.x). Override the JDK path with `JAVA=…` if your default
`java` is on a different major.

## Versioning

When Solaris upgrades to a new Mojang patch release, update
`.analysis/server.jar` to that release and rerun the extraction
script. Diff the resulting tree the same way you would any other
source-controlled directory.
