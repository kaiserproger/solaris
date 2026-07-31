# Loader Live-Gate Fixture

**Deployment: Requires Solaris Loader on client.** An unmodified vanilla client
is rejected during Configuration with the supported loader platforms and
required bundle identities.

This is the isolated two-owner input for the Solaris Loader real-client gate.
It uses production plugin discovery and artifact transfer; it does not alter a
PrismLauncher instance.

The tracked archives are reproducible from their inspectable sources:

```sh
tools/build-loader-live-gate-fixture.sh --check
```

Start the isolated server from the repository root:

```sh
cargo run --bin mc-server -- --config examples/loader-live-gate/playable.toml
```

Launch one Loader client with the separate MCP launcher, then connect it to
`127.0.0.1:25567`:

```sh
SOLARIS_CLIENT_MCP_TOKEN='<random-token>' \
SOLARIS_CLIENT_MCP_PORT=39110 \
SOLARIS_CLIENT_MCP_USERNAME=SolarisLoader \
tools/run-loader-client-mcp.sh fabric
```

After accepting the exact Loader permission prompt, run `/loader_ruby` and
`/loader_sapphire`. Each command opens its owner's screen and grants that
owner's block carrier. The screen displays the owner's custom item and button;
the button routes an owner-scoped interaction back to Lua. Place and break the
two granted blocks through ordinary play to exercise their distinct carrier
projection and presentation.

Repeat the same fixture later with `neoforge` and `forge` for the full visual
matrix. This fixture alone is not visual evidence.
