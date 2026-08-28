# Loader Live-Gate Fixture

**Deployment: Requires Solaris Loader on client.** A client profile without
Solaris Loader protocol support is rejected during Configuration with the
supported loader platforms and required bundle identities.

This is the isolated two-owner input for the Solaris Loader real-client gate.
It uses production plugin discovery and artifact transfer; it does not alter a
PrismLauncher instance.

The tracked archives are reproducible from their inspectable sources:

```sh
tools/build-loader-live-gate-fixture.sh --check
```

Run the automated real-client gate for any supported Loader platform:

```sh
python3 tools/run-loader-live-gate.py fabric
python3 tools/run-loader-live-gate.py neoforge
python3 tools/run-loader-live-gate.py forge
```

Each invocation creates an isolated Xvfb display, world, game directory,
permission/cache directory, server log, client log, and `result.json` below
`.analysis/loader-live-gate/runs/`. The gate starts the production server,
connects to `127.0.0.1:25567`, accepts the exact Loader permission prompt through
the embedded MCP endpoint, runs `/loader_ruby` and `/loader_sapphire`, presses
each owner's exact button, verifies the corresponding real inventory grants,
checks both exact bundle cache identities, and requires the client to remain in
Play.

For manual inspection, start the server with:

```sh
cargo run --bin mc-server -- --config examples/loader-live-gate/playable.toml
```

and launch one isolated client with a per-run MCP bearer token:

```sh
SOLARIS_CLIENT_MCP_TOKEN=change-me \
tools/run-loader-client-mcp.sh <fabric|neoforge|forge>
```

After accepting the permission prompt, `/loader_ruby` and `/loader_sapphire`
open owner-specific screens and grant their owner block carriers; placing and
breaking them exercises the distinct world projection and presentation path.

The complementary no-Loader compatibility gate is:

```sh
python3 tools/run-plugin-client-compat-gate.py
```

It proves that a server-only `basic-economy` plugin accepts the ordinary
real-client profile and opens its server-owned `/economy` menu, while the same
client profile without Solaris Loader is rejected by this client-required
fixture during Configuration with an explicit Loader-required reason.
