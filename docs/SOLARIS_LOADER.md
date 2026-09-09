# Install Solaris Loader on a client

Solaris Loader is an optional **client-side** mod for Solaris plugins that ship
verified UI (screens/HUD), assets, sounds, items, blocks, or UI/keyboard interactions. It is not
needed when every selected server plugin is `server_only`.

The current Loader prototype targets exactly **Minecraft Java Edition 26.1.2**
and **Java 25**. Use the adapter matching the mod loader in that Minecraft
instance; do not install more than one Solaris Loader adapter in the same
profile.

## Compatibility matrix

The repository currently builds and tests against:

| Client platform | Supported baseline for 26.1.2 | Solaris jar |
| --- | --- | --- |
| Fabric | Fabric Loader `0.19.3`, Fabric API `0.155.2+26.1.2` | `loader-fabric-0.1.0.jar` |
| NeoForge | NeoForge `26.1.2.76` | `loader-neoforge-0.1.0.jar` |
| Forge | Forge `26.1.2-64.1.0` | `loader-forge-0.1.0.jar` |

The mod metadata requires Minecraft 26.1.2. Fabric also requires Fabric API and
Fabric Loader 0.19.3 or newer; the validated baseline above is preferred.
NeoForge requires 26.1.2.76 or newer within the 26.1.2 line. Forge requires
loader major 64 within the 26.1 line. The exact versions in the table are the
field-tested development matrix; other newer compatible loader builds are not a
Solaris alpha release claim.

## Build the client jars

Prebuilt Loader publishing is not yet part of the released-alpha installer.
Build compatible sources from
[`solaris-loader`](https://github.com/kaiserproger/solaris-loader), checked out
beside the server repository. Their Git histories are independent: a server
tag or commit is not a Loader revision. Alpha protocol and bundle contracts
can change between revisions. Current Loader wire protocol is **2**, with no
protocol-1 compatibility path; plugin API is still `0.6.0`.
Declared keyboard actions leave vanilla movement, menus, screenshots and
fullscreen handling intact. They are suppressed in menus/overlays or without
window focus; bindings reset on reconnect. Rebinding, chords and mouse/gamepad
actions are not implemented. Sound bundles request `play_sounds` alongside
`load_assets`; they support personal and world-positioned one-shots, volume,
pitch and owner-local stop. Playback respects master volume and ends on
disconnect. Loops and moving sound sources are not implemented.
From that source checkout:

```sh
cd ../solaris-loader
./gradlew --no-configuration-cache \
  :loader-fabric:jar \
  :loader-neoforge:jar \
  :loader-forge:jar
```

Use only the jar for the chosen platform:

```text
loader-fabric/build/libs/loader-fabric-0.1.0.jar
loader-neoforge/build/libs/loader-neoforge-0.1.0.jar
loader-forge/build/libs/loader-forge-0.1.0.jar
```

These are client mods, not Solaris server plugins. Do not put them in the
server's `[plugins].directory`.

## Install in Fabric

1. Create/select a Minecraft **26.1.2** Fabric instance using Java 25.
2. Install Fabric Loader and Fabric API; use the validated versions above when
   reproducing a Solaris field test.
3. Open that instance's game directory and create `mods/` if needed.
4. Copy `loader-fabric-0.1.0.jar` into `mods/`.
5. Start that same instance and connect to the Solaris server.

For PrismLauncher, right-click the instance, choose **Folder**, and place the jar
in that folder's `minecraft/mods/` directory (or use the instance's Mods page).
Installing it in another instance or the global launcher directory has no
effect.

## Install in NeoForge

1. Create/select a Minecraft **26.1.2** NeoForge instance using Java 25.
2. Use NeoForge `26.1.2.76` for the validated matrix.
3. Copy `loader-neoforge-0.1.0.jar` into that instance's `mods/` directory.
4. Start the instance and connect.

Do not use the Fabric or Forge jar in a NeoForge profile.

## Install in Forge

1. Create/select a Minecraft **26.1.2** Forge instance using Java 25.
2. Use Forge `26.1.2-64.1.0` for the validated matrix.
3. Copy `loader-forge-0.1.0.jar` into that instance's `mods/` directory.
4. Start the instance and connect.

Do not use the NeoForge jar merely because Forge and NeoForge both use TOML mod
metadata; their networking adapters are different.

## First connection and permissions

When the server requires Loader content, Solaris sends a manifest during the
Minecraft Configuration phase. On the first connection for an exact normalized
server address and requested permission set, Loader opens a Minecraft
confirmation screen.

- **Allow:** Loader requests missing exact bundle identities, verifies each
  declared size and SHA-256, publishes it atomically in the cache, activates the
  permitted content, then completes login.
- **Deny:** no artifact is requested or staged, and the connection closes
  without acknowledgement.
- A different server address or changed permission set prompts again.
- A previous allow/deny decision is reused only for that address and exact
  permission set.

The default cache is:

```text
~/.solaris/loader-cache/
```

It contains `permissions.properties` plus content-addressed bundle data. The JVM
property below overrides the cache for an isolated profile:

```text
-Dsolaris.loader.cacheDir=/absolute/path/to/loader-cache
```

Do not copy a permission file between untrusted server addresses. Cached bundle
bytes are still checked against the server manifest before activation. The
server allows up to two minutes for this Loader-only confirmation/transfer
exchange.

## Required-client and disconnect behavior

A server with no client bundles sends no Loader payload and remains compatible
with an ordinary vanilla 26.1.2 client. If one or more selected plugins declare
`[client].bundles`, the deployment is `server_and_client` and Loader is required.

A missing Loader, wrong platform/version, denied permission, missing
acknowledgement, invalid bundle, size/hash mismatch, or activation failure stops
login during Configuration. The server disconnect reason names the supported
loader platforms and required bundle identities where available; Solaris does
not silently replace custom content with vanilla content.

Loader clears activated registries, HUD state, and transient resources when the
connection closes, so one server's content cannot be reused by a later
connection merely because the Minecraft process stayed open.

## How operators know Loader is required

Run the server check before deployment:

```sh
solaris --check --config server.toml
```

In `discovered_plugins`:

- `deployment: "server_only"` and empty `client_bundles` means an ordinary
  vanilla client is accepted by that plugin;
- `deployment: "server_and_client"` means Solaris Loader is required;
- `supported_loaders`, `permissions`, and `client_bundles` show exactly which
  adapters, permission set, identities, paths, sizes, and hashes are required.

Startup logs repeat the derived deployment for each discovered plugin. There is
no separate manifest switch that an operator can forget to keep synchronized.
See [Plugin deployment](PLUGINS.md#deployment-server-only-or-loader-required).

## Troubleshooting

### The client says Loader is required

Confirm the jar is in the `mods/` directory of the instance that was actually
launched, confirm Minecraft reports 26.1.2, and confirm the adapter matches
Fabric/NeoForge/Forge. Fabric also needs Fabric API. Check the client log for
`solaris_loader` loading before retrying.

### No permission prompt appears

A stored allow/deny decision may already match. Close Minecraft, inspect the
correct cache directory and `permissions.properties`, and remove only the entry
for the intended test server if you deliberately want to prompt again. Also
confirm the server check says `server_and_client`; server-only plugins do not
start a Loader handshake.

### The prompt was denied

Reconnect and change the stored decision for that exact server only after
reviewing the permissions with the server operator. Denial intentionally causes
a disconnect and no download.

### Bundle verification or activation fails

The operator should rerun `--check` and verify that each configured artifact
exists and exactly matches `size_bytes` and `sha256`. The client should not edit
cached ZIPs. Remove the failing exact cache identity and reconnect only after the
server artifact is corrected; changing artifact bytes requires a new matching
manifest identity.

### Platform/version mismatch

Use the compatibility table above. Do not rename one platform jar to make a
loader accept it. Capture both the Minecraft client log and Solaris disconnect
reason when reporting a failure.

## Developer compatibility gate

The repository includes a Loader-required fixture and isolated real-client
checks:

```sh
python3 -m tools.harness run loader-live --platform fabric
python3 -m tools.harness run loader-live --platform neoforge
python3 -m tools.harness run loader-live --platform forge
```

See [`../examples/loader-live-gate/README.md`](../examples/loader-live-gate/README.md)
for the fixture. These development gates are not an end-user auto-installer.
