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

Fixture generation requires `ffmpeg` with `libvorbis`; its deterministic mono
OGG sources are eight-second 440 Hz Ruby and 660 Hz Sapphire tones.

Run the automated real-client gate for any supported Loader platform:

```sh
python3 -m tools.harness run loader-live --platform fabric
python3 -m tools.harness run loader-live --platform neoforge
python3 -m tools.harness run loader-live --platform forge
```

Each invocation creates an isolated Xvfb display, world, game directory,
permission/cache directory, server log, client log, and `scenario.json` plus
the canonical harness `result.json` receipt below `.analysis/validation/`. The gate starts the production server,
connects to `127.0.0.1:25567`, accepts the exact Loader permission prompt through
the embedded MCP endpoint, runs `/loader_ruby` and `/loader_sapphire`, presses
each owner's exact button, verifies the corresponding real inventory grants,
checks both exact bundle cache identities, and requires the client to remain in
Play. It then presents both owner HUDs, updates Ruby, hides only Ruby, proves
ordinary jump input still works, and reconnects with the same cache.
Native Space also reports Ruby press/release. G reaches both owners; opening
the armed Ruby modal releases both actions before native key-up. Menu input
is suppressed, including declared Escape while closing inventory or a Loader
modal. Declared F2/F11 report both edges while vanilla writes a screenshot and
toggles fullscreen on/off. Mixed valid/invalid named-key batches in press and
respawn must fail before movement or plugin actions. Reconnect resets counts
and reinstalls bindings. Named input invokes Minecraft's actual callback, not
OS hardware emulation. Loader wire protocol 2 is required.
The sound gate additionally needs PulseAudio-compatible `pactl`/`parec`. It
creates a private null sink and selects it only in the isolated Minecraft profile.
It disables vanilla music there, records actual stereo 48 kHz float32 audio to
`audio.f32le`, and removes the sink on exit; the desktop default sink is untouched.
Spectral measurements cover
personal playback, quarter volume, pitch 1.5, near/mid/far world attenuation,
simultaneous owners, owner-local and foreign stop, disconnect silence,
reconnect silence and newly requested playback. `result.json` records each
sample's byte range, RMS and 440/660 Hz amplitudes; chat is an ordering barrier,
not proof of playback.
Screenshots under each run's `screenshots/` directory wait for the real loading
overlay to disappear. `passed` covers protocol, interaction, inventory, input
and measured audio checks; `ui_visual_review` explicitly remains required.
Inspect the images for both HUDs, updated text, owner-local removal and no stale HUD after reconnect
before claiming rendering/lifecycle acceptance.

The owner-approved Forge Xvfb profile sets `earlyWindowControl=false`: Minecraft
creates its normal game window instead of the failing FML early-window GL
context handoff. The real game renderer and HUD are still exercised; Forge's
early-window GL features are explicitly outside this gate's evidence.

For manual inspection, start the server with:

```sh
cargo run --bin mc-server -- --config examples/loader-live-gate/playable.toml
```

and launch one isolated client with a per-run MCP bearer token:

```sh
SOLARIS_CLIENT_MCP_TOKEN=change-me \
python3 -m tools.harness client --platform <fabric|neoforge|forge>
```

After accepting the permission prompt, `/loader_ruby` and `/loader_sapphire`
open owner-specific screens and grant their owner block carriers; placing and
breaking them exercises the distinct world projection and presentation path.
Add `hud`, `update`, or `hide` to either command to exercise the same UI resource
as a non-modal panel, change its text, or remove it. Screen-mode requests keep
their verified item/block display and interaction buttons.

`/loader_ruby input_status` and `/loader_sapphire input_status` report per-player
G/Space phase counts. `/loader_ruby input_modal` opens a modal on the next
declared press; `hud` disarms it. `/loader_ruby edge_status` reports Escape,
F2 and F11 counts. These server replies provide ordering barriers for negative
input assertions, rather than treating a timeout as success.

`/loader_ruby sound` and `/loader_sapphire sound` play their personal tones;
`sound_stop` stops that owner's tone. Ruby also provides `sound_quiet`,
`sound_pitch`, `sound_world x y z`, and `sound_foreign_stop` (which must not stop
Sapphire). These fixture commands call the public sound API rather than a
client test-only playback hook.

The complementary no-Loader compatibility gate is:

```sh
python3 -m tools.harness run core-client
```

It proves that a server-only `basic-economy` plugin accepts the ordinary
real-client profile and opens its server-owned `/economy` menu, while the same
client profile without Solaris Loader is rejected by this client-required
fixture during Configuration with an explicit Loader-required reason.
