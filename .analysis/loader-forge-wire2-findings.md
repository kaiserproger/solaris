# Forge payload decode findings (2026-07-24)

Scope: `client-mod/solaris-client-agent/loader-forge`

Target: capture-path test for Forge transport (`solaris:loader/manifest`) through `ClientboundCustomPayloadPacket.CONFIG_STREAM_CODEC`.

- Added/validated behavior in `ForgeLoaderTransportTest`: test `decodeCapturedManifestThroughConfigCodec` decodes captured manifest bytes, validates channel registration, and confirms:
  - `NetworkRegistry.findTarget(LoaderManifestPayload.TYPE.id()) != null`
  - decoded payload is not `DiscardedPayload`
  - decoded payload is `ForgePayload` with id `solaris:loader/manifest`
  - inner manifest JSON starts with `{"protocol":1`
- The capture fixture uses payload bytes only (without outer packet-id prefix), which is required by `ClientboundCustomPayloadPacket.CONFIG_STREAM_CODEC`.
- Verification command:
  - `./gradlew :loader-forge:test --tests 'dev.solaris.loader.forge.ForgeLoaderTransportTest.decodeCapturedManifestThroughConfigCodec'`
  - result: `BUILD SUCCESSFUL`
- Related log artifacts:
  - `.analysis/forge-loader-forge-transport-test.log`
  - `.analysis/forge-loader-forge-transport-test-debug.log`
  - `.analysis/forge-loader-forge-all-tests.log`
