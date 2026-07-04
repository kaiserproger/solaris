# Real-Client Client-Agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first invasive real-client automation path: a Fabric-style client-agent bridge plus a driver hook that can produce non-prepared real-client artifacts for the focused M94 rejected-block scenario.

**Architecture:** Add a Java/Gradle `client-mod/solaris-client-agent` subproject with a pure `bridge-core` module and a `fabric-agent` module. `bridge-core` owns the loopback HTTP/JSON RPC bridge, command protocol, fake-testable command handlers, and no Minecraft dependencies; `fabric-agent` adapts those handlers to the real Minecraft client thread. The existing `tools/run-real-client-regression.sh` remains the artifact runner and gains an agent-driver mode instead of being replaced.

**Tech Stack:** Java 25, Gradle Wrapper 9.6.0, Fabric Loom 1.17.11, Fabric Loader 0.19.3, official Mojang mappings through Loom, JUnit Jupiter 5.10.2, Gson 2.11.0, Python 3 stdlib for the external driver.

---

## File Structure

- Create `client-mod/solaris-client-agent/settings.gradle.kts` for the isolated Gradle build.
- Create `client-mod/solaris-client-agent/build.gradle.kts` for shared Java/JUnit/Gson settings.
- Create `client-mod/solaris-client-agent/gradle.properties` with pinned Gradle/Fabric defaults and an explicit `minecraftVersion=26.1.2`.
- Create `client-mod/solaris-client-agent/gradlew`, `gradlew.bat`, `gradle/wrapper/gradle-wrapper.properties`, and `gradle/wrapper/gradle-wrapper.jar`.
- Create `client-mod/solaris-client-agent/bridge-core/` for pure bridge protocol, HTTP transport, command registry, fakeable client facade, and unit tests.
- Create `client-mod/solaris-client-agent/fabric-agent/` for Fabric entrypoint, resources, and Minecraft client adapters.
- Create `tools/real-client-agent-driver.py` for the external JSON RPC driver.
- Modify `tools/run-real-client-regression.sh` to call the driver when agent env vars are present.
- Modify `docs/real-client-regression/README.md`, `docs/real-client-regression/manifests/m94-regression-pack.json`, and `crates/mc-test-harness/tests/real_client_manifest.rs` to document and guard the agent-run path.
- Modify `docs/VALIDATION_LEDGER.md` and `docs/REPLACEMENT_READINESS.md` only after a real client run exists; scaffold-only work must not improve readiness labels.

## Task 1: Gradle Wrapper And Build Skeleton

**Files:**
- Create: `client-mod/solaris-client-agent/settings.gradle.kts`
- Create: `client-mod/solaris-client-agent/build.gradle.kts`
- Create: `client-mod/solaris-client-agent/gradle.properties`
- Create: `client-mod/solaris-client-agent/gradlew`
- Create: `client-mod/solaris-client-agent/gradlew.bat`
- Create: `client-mod/solaris-client-agent/gradle/wrapper/gradle-wrapper.properties`
- Create: `client-mod/solaris-client-agent/gradle/wrapper/gradle-wrapper.jar`
- Create: `client-mod/solaris-client-agent/bridge-core/build.gradle.kts`

- [ ] **Step 1: Create the Gradle project files**

`client-mod/solaris-client-agent/settings.gradle.kts`:

```kotlin
pluginManagement {
    repositories {
        maven("https://maven.fabricmc.net/")
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        maven("https://maven.fabricmc.net/")
        mavenCentral()
    }
}

rootProject.name = "solaris-client-agent"

include("bridge-core")
include("fabric-agent")
```

`client-mod/solaris-client-agent/build.gradle.kts`:

```kotlin
plugins {
    java
    id("fabric-loom") version "1.17.11" apply false
}

allprojects {
    group = "dev.solaris"
    version = "0.1.0"
}

subprojects {
    plugins.withType<JavaPlugin> {
        extensions.configure<JavaPluginExtension> {
            toolchain {
                languageVersion.set(JavaLanguageVersion.of(25))
            }
        }
    }

    tasks.withType<Test>().configureEach {
        useJUnitPlatform()
    }
}
```

`client-mod/solaris-client-agent/gradle.properties`:

```properties
org.gradle.jvmargs=-Xmx2G
org.gradle.parallel=true

minecraftVersion=26.1.2
fabricLoaderVersion=0.19.3
```

`client-mod/solaris-client-agent/bridge-core/build.gradle.kts`:

```kotlin
plugins {
    java
}

dependencies {
    implementation("com.google.code.gson:gson:2.11.0")
    testImplementation("org.junit.jupiter:junit-jupiter-api:5.10.2")
    testRuntimeOnly("org.junit.jupiter:junit-jupiter-engine:5.10.2")
}
```

- [ ] **Step 2: Add the Gradle Wrapper**

Run:

```bash
cd client-mod/solaris-client-agent
mkdir -p gradle/wrapper
curl -fsSL -o gradle/wrapper/gradle-wrapper.jar \
  https://services.gradle.org/distributions/gradle-9.6.0-wrapper.jar
printf '%s  %s\n' \
  '497c8c2a7e5031f6aa847f88104aa80a93532ec32ee17bdb8d1d2f67a194a9c7' \
  'gradle/wrapper/gradle-wrapper.jar' | sha256sum -c -
```

`client-mod/solaris-client-agent/gradle/wrapper/gradle-wrapper.properties`:

```properties
distributionBase=GRADLE_USER_HOME
distributionPath=wrapper/dists
distributionUrl=https\://services.gradle.org/distributions/gradle-9.6.0-bin.zip
networkTimeout=10000
validateDistributionUrl=true
zipStoreBase=GRADLE_USER_HOME
zipStorePath=wrapper/dists
```

`client-mod/solaris-client-agent/gradlew`:

```sh
#!/bin/sh
APP_NAME="Gradle"
APP_BASE_NAME=`basename "$0"`
DEFAULT_JVM_OPTS=""
MAX_FD="maximum"
warn () { echo "$*"; }
die () { echo "$*"; exit 1; }
cygwin=false
msys=false
darwin=false
nonstop=false
case "`uname`" in
  CYGWIN* ) cygwin=true ;;
  Darwin* ) darwin=true ;;
  MSYS* | MINGW* ) msys=true ;;
  NONSTOP* ) nonstop=true ;;
esac
PRG="$0"
while [ -h "$PRG" ] ; do
  ls=`ls -ld "$PRG"`
  link=`expr "$ls" : '.*-> \(.*\)$'`
  if expr "$link" : '/.*' > /dev/null; then
    PRG="$link"
  else
    PRG=`dirname "$PRG"`"/$link"
  fi
done
SAVED="`pwd`"
APP_HOME="`dirname "$PRG"`/"
APP_HOME=`cd "$APP_HOME" >/dev/null && pwd -P`
cd "$SAVED" >/dev/null
CLASSPATH=$APP_HOME/gradle/wrapper/gradle-wrapper.jar
if [ -n "$JAVA_HOME" ] ; then
  JAVACMD="$JAVA_HOME/bin/java"
else
  JAVACMD="java"
fi
if [ ! -x "$JAVACMD" ] ; then
  die "ERROR: JAVA_HOME is set to an invalid directory: $JAVA_HOME"
fi
exec "$JAVACMD" $DEFAULT_JVM_OPTS $JAVA_OPTS $GRADLE_OPTS \
  -classpath "$CLASSPATH" org.gradle.wrapper.GradleWrapperMain "$@"
```

Set executable bit:

```bash
chmod +x client-mod/solaris-client-agent/gradlew
```

`client-mod/solaris-client-agent/gradlew.bat`:

```bat
@echo off
set APP_HOME=%~dp0
set CLASSPATH=%APP_HOME%\gradle\wrapper\gradle-wrapper.jar
set JAVACMD=java
"%JAVACMD%" %JAVA_OPTS% %GRADLE_OPTS% -classpath "%CLASSPATH%" org.gradle.wrapper.GradleWrapperMain %*
```

- [ ] **Step 3: Verify the skeleton**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :bridge-core:tasks --no-daemon
```

Expected: command exits 0 and lists `bridge-core` tasks. If Gradle cannot download dependencies, stop and report `blocked-client-tooling`; do not mark real-client automation as available.

- [ ] **Step 4: Commit**

```bash
git add client-mod/solaris-client-agent/settings.gradle.kts \
  client-mod/solaris-client-agent/build.gradle.kts \
  client-mod/solaris-client-agent/gradle.properties \
  client-mod/solaris-client-agent/gradlew \
  client-mod/solaris-client-agent/gradlew.bat \
  client-mod/solaris-client-agent/gradle/wrapper/gradle-wrapper.properties \
  client-mod/solaris-client-agent/gradle/wrapper/gradle-wrapper.jar \
  client-mod/solaris-client-agent/bridge-core/build.gradle.kts
git commit -m "build: scaffold real-client agent project"
```

## Task 2: Bridge Protocol DTOs And JSON Codec

**Files:**
- Create: `client-mod/solaris-client-agent/bridge-core/src/test/java/dev/solaris/agent/bridge/BridgeCodecTest.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/bridge/BridgeRequest.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/bridge/BridgeResponse.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/bridge/BridgeError.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/bridge/BridgeCodec.java`

- [ ] **Step 1: Write the failing codec test**

`BridgeCodecTest.java`:

```java
package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class BridgeCodecTest {
    @Test
    void decodesRequestWithIdCommandSecretAndPayload() {
        BridgeRequest request = BridgeCodec.decodeRequest("""
            {"id":7,"secret":"run-secret","command":"ping","payload":{"client":"probe"}}
            """);

        assertEquals(7L, request.id());
        assertEquals("run-secret", request.secret());
        assertEquals("ping", request.command());
        assertEquals("probe", request.payload().get("client").getAsString());
    }

    @Test
    void encodesSuccessResponseWithPayload() {
        JsonObject payload = new JsonObject();
        payload.addProperty("bridge_version", "0.1.0");

        String encoded = BridgeCodec.encodeResponse(BridgeResponse.success(8L, payload));

        assertTrue(encoded.contains("\"id\":8"));
        assertTrue(encoded.contains("\"ok\":true"));
        assertTrue(encoded.contains("\"bridge_version\":\"0.1.0\""));
        assertFalse(encoded.contains("\"error\""));
    }

    @Test
    void encodesStructuredErrorResponse() {
        BridgeError error = new BridgeError("unknown-command", "unsupported command: mine");

        String encoded = BridgeCodec.encodeResponse(BridgeResponse.failure(9L, error));

        assertTrue(encoded.contains("\"id\":9"));
        assertTrue(encoded.contains("\"ok\":false"));
        assertTrue(encoded.contains("\"code\":\"unknown-command\""));
        assertTrue(encoded.contains("\"message\":\"unsupported command: mine\""));
    }
}
```

- [ ] **Step 2: Run RED**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :bridge-core:test --tests dev.solaris.agent.bridge.BridgeCodecTest --no-daemon
```

Expected: FAIL at compile time because `BridgeRequest`, `BridgeResponse`, `BridgeError`, and `BridgeCodec` do not exist.

- [ ] **Step 3: Implement the codec**

`BridgeRequest.java`:

```java
package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;

public record BridgeRequest(long id, String secret, String command, JsonObject payload) {
}
```

`BridgeError.java`:

```java
package dev.solaris.agent.bridge;

public record BridgeError(String code, String message) {
}
```

`BridgeResponse.java`:

```java
package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;

public record BridgeResponse(long id, boolean ok, JsonObject payload, BridgeError error) {
    public static BridgeResponse success(long id, JsonObject payload) {
        return new BridgeResponse(id, true, payload, null);
    }

    public static BridgeResponse failure(long id, BridgeError error) {
        return new BridgeResponse(id, false, new JsonObject(), error);
    }
}
```

`BridgeCodec.java`:

```java
package dev.solaris.agent.bridge;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;

public final class BridgeCodec {
    private static final Gson GSON = new Gson();

    private BridgeCodec() {
    }

    public static BridgeRequest decodeRequest(String body) {
        JsonObject root = JsonParser.parseString(body).getAsJsonObject();
        JsonObject payload = root.has("payload") && root.get("payload").isJsonObject()
            ? root.getAsJsonObject("payload")
            : new JsonObject();
        return new BridgeRequest(
            root.get("id").getAsLong(),
            root.get("secret").getAsString(),
            root.get("command").getAsString(),
            payload
        );
    }

    public static String encodeResponse(BridgeResponse response) {
        return GSON.toJson(response);
    }
}
```

- [ ] **Step 4: Run GREEN**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :bridge-core:test --tests dev.solaris.agent.bridge.BridgeCodecTest --no-daemon
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add client-mod/solaris-client-agent/bridge-core/src/main/java \
  client-mod/solaris-client-agent/bridge-core/src/test/java
git commit -m "feat: add client-agent bridge codec"
```

## Task 3: Loopback HTTP Bridge

**Files:**
- Create: `client-mod/solaris-client-agent/bridge-core/src/test/java/dev/solaris/agent/bridge/AgentHttpBridgeTest.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/bridge/BridgeCommand.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/bridge/CommandRegistry.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/bridge/AgentHttpBridge.java`

- [ ] **Step 1: Write the failing HTTP bridge test**

`AgentHttpBridgeTest.java`:

```java
package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class AgentHttpBridgeTest {
    @Test
    void acceptsLoopbackRpcWithSharedSecret() throws Exception {
        CommandRegistry registry = new CommandRegistry();
        registry.register("ping", request -> {
            JsonObject payload = new JsonObject();
            payload.addProperty("pong", true);
            return payload;
        });

        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, registry)) {
            HttpResponse<String> response = post(bridge.port(), """
                {"id":1,"secret":"run-secret","command":"ping","payload":{}}
                """);

            assertEquals(200, response.statusCode());
            assertTrue(response.body().contains("\"ok\":true"));
            assertTrue(response.body().contains("\"pong\":true"));
        }
    }

    @Test
    void rejectsMissingSecretWithoutExecutingCommand() throws Exception {
        CommandRegistry registry = new CommandRegistry();
        registry.register("ping", request -> {
            throw new AssertionError("command must not execute without secret");
        });

        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, registry)) {
            HttpResponse<String> response = post(bridge.port(), """
                {"id":2,"secret":"wrong","command":"ping","payload":{}}
                """);

            assertEquals(403, response.statusCode());
            assertTrue(response.body().contains("\"ok\":false"));
            assertTrue(response.body().contains("\"code\":\"forbidden\""));
        }
    }

    @Test
    void rejectsUnknownCommandAsStructuredError() throws Exception {
        try (AgentHttpBridge bridge = AgentHttpBridge.start("run-secret", 0, new CommandRegistry())) {
            HttpResponse<String> response = post(bridge.port(), """
                {"id":3,"secret":"run-secret","command":"mine","payload":{}}
                """);

            assertEquals(404, response.statusCode());
            assertTrue(response.body().contains("\"ok\":false"));
            assertTrue(response.body().contains("\"code\":\"unknown-command\""));
        }
    }

    private static HttpResponse<String> post(int port, String body) throws Exception {
        HttpRequest request = HttpRequest.newBuilder()
            .uri(URI.create("http://127.0.0.1:" + port + "/rpc"))
            .header("Content-Type", "application/json")
            .POST(HttpRequest.BodyPublishers.ofString(body))
            .build();
        return HttpClient.newHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
    }
}
```

- [ ] **Step 2: Run RED**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :bridge-core:test --tests dev.solaris.agent.bridge.AgentHttpBridgeTest --no-daemon
```

Expected: FAIL at compile time because `AgentHttpBridge`, `CommandRegistry`, and `BridgeCommand` do not exist.

- [ ] **Step 3: Implement loopback HTTP bridge**

`BridgeCommand.java`:

```java
package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;

@FunctionalInterface
public interface BridgeCommand {
    JsonObject execute(BridgeRequest request) throws Exception;
}
```

`CommandRegistry.java`:

```java
package dev.solaris.agent.bridge;

import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Optional;

public final class CommandRegistry {
    private final Map<String, BridgeCommand> commands = new LinkedHashMap<>();

    public void register(String name, BridgeCommand command) {
        commands.put(name, command);
    }

    public Optional<BridgeCommand> find(String name) {
        return Optional.ofNullable(commands.get(name));
    }
}
```

`AgentHttpBridge.java`:

```java
package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public final class AgentHttpBridge implements AutoCloseable {
    private final HttpServer server;
    private final ExecutorService executor;

    private AgentHttpBridge(HttpServer server, ExecutorService executor) {
        this.server = server;
        this.executor = executor;
    }

    public static AgentHttpBridge start(String secret, int port, CommandRegistry registry) throws IOException {
        if (secret == null || secret.isBlank()) {
            throw new IllegalArgumentException("shared secret must not be blank");
        }
        HttpServer server = HttpServer.create(new InetSocketAddress(InetAddress.getLoopbackAddress(), port), 0);
        ExecutorService executor = Executors.newSingleThreadExecutor(runnable -> {
            Thread thread = new Thread(runnable, "solaris-client-agent-bridge");
            thread.setDaemon(true);
            return thread;
        });
        server.setExecutor(executor);
        server.createContext("/rpc", exchange -> handle(exchange, secret, registry));
        server.start();
        return new AgentHttpBridge(server, executor);
    }

    public int port() {
        return server.getAddress().getPort();
    }

    private static void handle(HttpExchange exchange, String secret, CommandRegistry registry) throws IOException {
        if (!"POST".equals(exchange.getRequestMethod())) {
            write(exchange, 405, BridgeCodec.encodeResponse(
                BridgeResponse.failure(0, new BridgeError("method-not-allowed", "POST required"))));
            return;
        }

        BridgeRequest request;
        try {
            String body = new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
            request = BridgeCodec.decodeRequest(body);
        } catch (RuntimeException error) {
            write(exchange, 400, BridgeCodec.encodeResponse(
                BridgeResponse.failure(0, new BridgeError("bad-request", error.getMessage()))));
            return;
        }

        if (!secret.equals(request.secret())) {
            write(exchange, 403, BridgeCodec.encodeResponse(
                BridgeResponse.failure(request.id(), new BridgeError("forbidden", "shared secret mismatch"))));
            return;
        }

        BridgeCommand command = registry.find(request.command()).orElse(null);
        if (command == null) {
            write(exchange, 404, BridgeCodec.encodeResponse(BridgeResponse.failure(
                request.id(), new BridgeError("unknown-command", "unsupported command: " + request.command()))));
            return;
        }

        try {
            JsonObject payload = command.execute(request);
            write(exchange, 200, BridgeCodec.encodeResponse(BridgeResponse.success(request.id(), payload)));
        } catch (Exception error) {
            write(exchange, 500, BridgeCodec.encodeResponse(BridgeResponse.failure(
                request.id(), new BridgeError("command-failed", error.getMessage()))));
        }
    }

    private static void write(HttpExchange exchange, int status, String body) throws IOException {
        byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().set("Content-Type", "application/json; charset=utf-8");
        exchange.sendResponseHeaders(status, bytes.length);
        exchange.getResponseBody().write(bytes);
        exchange.close();
    }

    @Override
    public void close() {
        server.stop(0);
        executor.shutdownNow();
    }
}
```

- [ ] **Step 4: Run GREEN**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :bridge-core:test --tests dev.solaris.agent.bridge.AgentHttpBridgeTest --no-daemon
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add client-mod/solaris-client-agent/bridge-core/src/main/java \
  client-mod/solaris-client-agent/bridge-core/src/test/java
git commit -m "feat: add loopback client-agent bridge"
```

## Task 4: Client Facade Commands With Fakeable Executor

**Files:**
- Create: `client-mod/solaris-client-agent/bridge-core/src/test/java/dev/solaris/agent/bridge/ClientCommandsTest.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/client/ClientTaskExecutor.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/client/ClientFacade.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/client/ClientSnapshot.java`
- Create: `client-mod/solaris-client-agent/bridge-core/src/main/java/dev/solaris/agent/client/ClientCommands.java`

- [ ] **Step 1: Write the failing command test**

`ClientCommandsTest.java`:

```java
package dev.solaris.agent.bridge;

import dev.solaris.agent.client.ClientCommands;
import dev.solaris.agent.client.ClientFacade;
import dev.solaris.agent.client.ClientSnapshot;
import dev.solaris.agent.client.ClientTaskExecutor;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
import java.util.concurrent.Callable;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class ClientCommandsTest {
    @Test
    void pingReportsBridgeVersionWithoutClientThread() throws Exception {
        CommandRegistry registry = ClientCommands.create(new ImmediateExecutor(), new FakeClient(), Path.of("run"));

        BridgeCommand ping = registry.find("ping").orElseThrow();

        assertEquals("0.1.0", ping.execute(request("ping")).get("bridge_version").getAsString());
    }

    @Test
    void stateRunsThroughClientThreadExecutor() throws Exception {
        ImmediateExecutor executor = new ImmediateExecutor();
        CommandRegistry registry = ClientCommands.create(executor, new FakeClient(), Path.of("run"));

        BridgeCommand state = registry.find("state").orElseThrow();

        assertEquals("minecraft:overworld", state.execute(request("state")).get("dimension").getAsString());
        assertEquals(1, executor.calls);
    }

    private static BridgeRequest request(String command) {
        return BridgeCodec.decodeRequest("{\"id\":1,\"secret\":\"s\",\"command\":\"" + command + "\",\"payload\":{}}");
    }

    private static final class ImmediateExecutor implements ClientTaskExecutor {
        int calls;

        @Override
        public <T> T callOnClientThread(Callable<T> callable) throws Exception {
            calls += 1;
            return callable.call();
        }
    }

    private static final class FakeClient implements ClientFacade {
        @Override
        public ClientSnapshot snapshot() {
            return new ClientSnapshot(
                true,
                "minecraft:overworld",
                10.0,
                64.0,
                -3.0,
                2,
                "none",
                ""
            );
        }

        @Override
        public void connect(String host, int port) {
        }

        @Override
        public void selectHotbarSlot(int slot) {
        }

        @Override
        public void lookAtBlock(int x, int y, int z, String face) {
        }

        @Override
        public void useItemOn(int x, int y, int z, String face) {
        }

        @Override
        public Path screenshot(Path runDirectory, String name) {
            assertTrue(name.endsWith(".png"));
            return runDirectory.resolve(name);
        }

        @Override
        public void disconnect() {
        }
    }
}
```

- [ ] **Step 2: Run RED**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :bridge-core:test --tests dev.solaris.agent.bridge.ClientCommandsTest --no-daemon
```

Expected: FAIL at compile time because the client facade classes do not exist.

- [ ] **Step 3: Implement client command abstractions**

`ClientTaskExecutor.java`:

```java
package dev.solaris.agent.client;

import java.util.concurrent.Callable;

public interface ClientTaskExecutor {
    <T> T callOnClientThread(Callable<T> callable) throws Exception;
}
```

`ClientSnapshot.java`:

```java
package dev.solaris.agent.client;

public record ClientSnapshot(
    boolean inPlay,
    String dimension,
    double x,
    double y,
    double z,
    int selectedHotbarSlot,
    String currentScreen,
    String disconnectReason
) {
}
```

`ClientFacade.java`:

```java
package dev.solaris.agent.client;

import java.nio.file.Path;

public interface ClientFacade {
    ClientSnapshot snapshot();
    void connect(String host, int port);
    void selectHotbarSlot(int slot);
    void lookAtBlock(int x, int y, int z, String face);
    void useItemOn(int x, int y, int z, String face);
    Path screenshot(Path runDirectory, String name);
    void disconnect();
}
```

`ClientCommands.java`:

```java
package dev.solaris.agent.client;

import com.google.gson.JsonObject;
import dev.solaris.agent.bridge.CommandRegistry;

import java.nio.file.Path;

public final class ClientCommands {
    private ClientCommands() {
    }

    public static CommandRegistry create(ClientTaskExecutor executor, ClientFacade client, Path runDirectory) {
        CommandRegistry registry = new CommandRegistry();
        registry.register("ping", request -> {
            JsonObject payload = new JsonObject();
            payload.addProperty("bridge_version", "0.1.0");
            payload.addProperty("agent", "solaris-client-agent");
            return payload;
        });
        registry.register("state", request -> executor.callOnClientThread(() -> snapshotJson(client.snapshot())));
        registry.register("connect", request -> executor.callOnClientThread(() -> {
            JsonObject payload = request.payload();
            client.connect(
                payload.has("host") ? payload.get("host").getAsString() : "127.0.0.1",
                payload.has("port") ? payload.get("port").getAsInt() : 25565
            );
            return ok();
        }));
        registry.register("set_hotbar_slot", request -> executor.callOnClientThread(() -> {
            client.selectHotbarSlot(request.payload().get("slot").getAsInt());
            return ok();
        }));
        registry.register("look_at_block", request -> executor.callOnClientThread(() -> {
            JsonObject payload = request.payload();
            client.lookAtBlock(
                payload.get("x").getAsInt(),
                payload.get("y").getAsInt(),
                payload.get("z").getAsInt(),
                payload.get("face").getAsString()
            );
            return ok();
        }));
        registry.register("use_item_on", request -> executor.callOnClientThread(() -> {
            JsonObject payload = request.payload();
            client.useItemOn(
                payload.get("x").getAsInt(),
                payload.get("y").getAsInt(),
                payload.get("z").getAsInt(),
                payload.get("face").getAsString()
            );
            return ok();
        }));
        registry.register("screenshot", request -> executor.callOnClientThread(() -> {
            String name = request.payload().has("name")
                ? request.payload().get("name").getAsString()
                : "screenshot.png";
            JsonObject response = ok();
            response.addProperty("path", client.screenshot(runDirectory, name).toString());
            return response;
        }));
        registry.register("disconnect", request -> executor.callOnClientThread(() -> {
            client.disconnect();
            return ok();
        }));
        return registry;
    }

    private static JsonObject snapshotJson(ClientSnapshot snapshot) {
        JsonObject payload = new JsonObject();
        payload.addProperty("in_play", snapshot.inPlay());
        payload.addProperty("dimension", snapshot.dimension());
        payload.addProperty("x", snapshot.x());
        payload.addProperty("y", snapshot.y());
        payload.addProperty("z", snapshot.z());
        payload.addProperty("selected_hotbar_slot", snapshot.selectedHotbarSlot());
        payload.addProperty("current_screen", snapshot.currentScreen());
        payload.addProperty("disconnect_reason", snapshot.disconnectReason());
        return payload;
    }

    private static JsonObject ok() {
        JsonObject payload = new JsonObject();
        payload.addProperty("status", "ok");
        return payload;
    }
}
```

- [ ] **Step 4: Run GREEN**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :bridge-core:test --tests dev.solaris.agent.bridge.ClientCommandsTest --no-daemon
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add client-mod/solaris-client-agent/bridge-core/src/main/java \
  client-mod/solaris-client-agent/bridge-core/src/test/java
git commit -m "feat: add client-agent command facade"
```

## Task 5: Fabric Agent Entrypoint

**Files:**
- Create: `client-mod/solaris-client-agent/fabric-agent/build.gradle.kts`
- Create: `client-mod/solaris-client-agent/fabric-agent/src/main/resources/fabric.mod.json`
- Create: `client-mod/solaris-client-agent/fabric-agent/src/main/resources/solaris-client-agent.mixins.json`
- Create: `client-mod/solaris-client-agent/fabric-agent/src/main/java/dev/solaris/agent/fabric/SolarisClientAgentMod.java`
- Create: `client-mod/solaris-client-agent/fabric-agent/src/main/java/dev/solaris/agent/fabric/MinecraftClientExecutor.java`
- Create: `client-mod/solaris-client-agent/fabric-agent/src/main/java/dev/solaris/agent/fabric/MinecraftClientFacade.java`

- [ ] **Step 1: Add resource shape test before implementation**

Run:

```bash
test -f client-mod/solaris-client-agent/fabric-agent/src/main/resources/fabric.mod.json
```

Expected: FAIL because the Fabric resources do not exist yet.

- [ ] **Step 2: Create the Fabric build and resources**

`fabric-agent/build.gradle.kts`:

```kotlin
plugins {
    id("fabric-loom")
    java
}

dependencies {
    minecraft("com.mojang:minecraft:${property("minecraftVersion")}")
    mappings(loom.officialMojangMappings())
    modImplementation("net.fabricmc:fabric-loader:${property("fabricLoaderVersion")}")
    implementation(project(":bridge-core"))
}
```

`fabric.mod.json`:

```json
{
  "schemaVersion": 1,
  "id": "solaris-client-agent",
  "version": "0.1.0",
  "name": "Solaris Client Agent",
  "description": "Loopback-only automation bridge for Solaris real-client validation.",
  "environment": "client",
  "entrypoints": {
    "client": [
      "dev.solaris.agent.fabric.SolarisClientAgentMod"
    ]
  },
  "mixins": [
    "solaris-client-agent.mixins.json"
  ],
  "depends": {
    "fabricloader": ">=0.19.3",
    "minecraft": "26.1.2"
  }
}
```

`solaris-client-agent.mixins.json`:

```json
{
  "required": false,
  "package": "dev.solaris.agent.fabric.mixin",
  "compatibilityLevel": "JAVA_25",
  "client": [],
  "injectors": {
    "defaultRequire": 1
  }
}
```

- [ ] **Step 3: Verify resource shape**

Run:

```bash
python3 -m json.tool client-mod/solaris-client-agent/fabric-agent/src/main/resources/fabric.mod.json >/dev/null
python3 -m json.tool client-mod/solaris-client-agent/fabric-agent/src/main/resources/solaris-client-agent.mixins.json >/dev/null
```

Expected: PASS.

- [ ] **Step 4: Add minimal Fabric entrypoint**

`SolarisClientAgentMod.java`:

```java
package dev.solaris.agent.fabric;

import dev.solaris.agent.bridge.AgentHttpBridge;
import dev.solaris.agent.client.ClientCommands;
import net.fabricmc.api.ClientModInitializer;

import java.nio.file.Path;

public final class SolarisClientAgentMod implements ClientModInitializer {
    private AgentHttpBridge bridge;

    @Override
    public void onInitializeClient() {
        String secret = System.getProperty("solaris.clientAgent.secret", "");
        if (secret.isBlank()) {
            return;
        }
        int port = Integer.getInteger("solaris.clientAgent.port", 39094);
        Path runDirectory = Path.of(System.getProperty("solaris.clientAgent.runDir", "."));
        try {
            bridge = AgentHttpBridge.start(
                secret,
                port,
                ClientCommands.create(new MinecraftClientExecutor(), new MinecraftClientFacade(), runDirectory)
            );
        } catch (Exception error) {
            throw new IllegalStateException("failed to start Solaris client-agent bridge", error);
        }
    }
}
```

`MinecraftClientExecutor.java`:

```java
package dev.solaris.agent.fabric;

import dev.solaris.agent.client.ClientTaskExecutor;
import net.minecraft.client.Minecraft;

import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;

public final class MinecraftClientExecutor implements ClientTaskExecutor {
    @Override
    public <T> T callOnClientThread(Callable<T> callable) throws Exception {
        Minecraft minecraft = Minecraft.getInstance();
        if (minecraft.isSameThread()) {
            return callable.call();
        }
        CompletableFuture<T> future = new CompletableFuture<>();
        minecraft.execute(() -> {
            try {
                future.complete(callable.call());
            } catch (Exception error) {
                future.completeExceptionally(error);
            }
        });
        return future.get();
    }
}
```

`MinecraftClientFacade.java`:

```java
package dev.solaris.agent.fabric;

import dev.solaris.agent.client.ClientFacade;
import dev.solaris.agent.client.ClientSnapshot;
import net.minecraft.client.Minecraft;

import java.nio.file.Path;

public final class MinecraftClientFacade implements ClientFacade {
    @Override
    public ClientSnapshot snapshot() {
        Minecraft minecraft = Minecraft.getInstance();
        boolean inPlay = minecraft.player != null && minecraft.level != null;
        String dimension = minecraft.level == null
            ? ""
            : minecraft.level.dimension().location().toString();
        String screen = minecraft.screen == null ? "none" : minecraft.screen.getClass().getName();
        return new ClientSnapshot(
            inPlay,
            dimension,
            minecraft.player == null ? 0.0 : minecraft.player.getX(),
            minecraft.player == null ? 0.0 : minecraft.player.getY(),
            minecraft.player == null ? 0.0 : minecraft.player.getZ(),
            minecraft.player == null ? -1 : minecraft.player.getInventory().selected,
            screen,
            ""
        );
    }

    @Override
    public void connect(String host, int port) {
        throw new UnsupportedOperationException("connect command is wired in the next task");
    }

    @Override
    public void selectHotbarSlot(int slot) {
        Minecraft.getInstance().player.getInventory().selected = slot;
    }

    @Override
    public void lookAtBlock(int x, int y, int z, String face) {
        throw new UnsupportedOperationException("look_at_block command is wired in the scenario task");
    }

    @Override
    public void useItemOn(int x, int y, int z, String face) {
        throw new UnsupportedOperationException("use_item_on command is wired in the scenario task");
    }

    @Override
    public Path screenshot(Path runDirectory, String name) {
        throw new UnsupportedOperationException("screenshot command is wired in the scenario task");
    }

    @Override
    public void disconnect() {
        Minecraft.getInstance().disconnect();
    }
}
```

- [ ] **Step 5: Compile the Fabric module**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :fabric-agent:compileJava --no-daemon
```

Expected: PASS if Fabric Loom can resolve Minecraft `26.1.2` and official mappings. If it fails because the version is unavailable from Mojang/Fabric metadata, record `blocked-client-tooling: Fabric metadata for 26.1.2 unavailable`, keep `bridge-core` green, and do not claim a runnable client mod.

- [ ] **Step 6: Commit**

```bash
git add client-mod/solaris-client-agent/fabric-agent
git commit -m "feat: add Fabric client-agent entrypoint"
```

## Task 6: External Agent Driver

**Files:**
- Create: `tools/tests/test_real_client_agent_driver.py`
- Create: `tools/real-client-agent-driver.py`

- [ ] **Step 1: Write the failing Python driver tests**

`tools/tests/test_real_client_agent_driver.py`:

```python
import json
import tempfile
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from threading import Thread

from tools.real_client_agent_driver import AgentClient, write_observations


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers["Content-Length"])
        body = json.loads(self.rfile.read(length))
        payload = {"echo": body["command"]}
        if body["command"] == "state":
            payload = {"in_play": True, "dimension": "minecraft:overworld", "selected_hotbar_slot": 0}
        response = {"id": body["id"], "ok": True, "payload": payload, "error": None}
        encoded = json.dumps(response).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, format, *args):
        pass


class AgentDriverTest(unittest.TestCase):
    def test_rpc_increments_ids_and_sends_secret(self):
        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            client = AgentClient("127.0.0.1", server.server_port, "secret")
            self.assertEqual({"echo": "ping"}, client.call("ping", {}))
            self.assertEqual("minecraft:overworld", client.call("state", {})["dimension"])
        finally:
            server.shutdown()

    def test_write_observations_creates_agent_run_result(self):
        with tempfile.TemporaryDirectory() as root:
            run_dir = Path(root)
            write_observations(run_dir, "m94-02b-rejected-block-resync", [{"command": "ping"}], "passed")
            observations = json.loads((run_dir / "observations.json").read_text())

        self.assertEqual("agent-run-real-client", observations["client_gate"])
        self.assertEqual("stabilization", observations["quality_label"])
        self.assertEqual("passed", observations["result"])
        self.assertEqual("m94-02b-rejected-block-resync", observations["scenarios"][0]["id"])


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run RED**

Run:

```bash
python3 -m unittest tools.tests.test_real_client_agent_driver -v
```

Expected: FAIL because `tools/real_client_agent_driver.py` does not exist.

- [ ] **Step 3: Implement the driver library and CLI**

`tools/real-client-agent-driver.py`:

```python
#!/usr/bin/env python3
import argparse
import json
import time
import urllib.request
from pathlib import Path


class AgentClient:
    def __init__(self, host: str, port: int, secret: str):
        self.url = f"http://{host}:{port}/rpc"
        self.secret = secret
        self.next_id = 1

    def call(self, command: str, payload: dict, timeout: float = 10.0) -> dict:
        request_id = self.next_id
        self.next_id += 1
        body = json.dumps({
            "id": request_id,
            "secret": self.secret,
            "command": command,
            "payload": payload,
        }).encode()
        request = urllib.request.Request(
            self.url,
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=timeout) as response:
            decoded = json.loads(response.read().decode())
        if not decoded.get("ok"):
            error = decoded.get("error") or {}
            raise RuntimeError(f"{error.get('code', 'agent-error')}: {error.get('message', '')}")
        return decoded["payload"]


def wait_play(client: AgentClient, timeout_seconds: float) -> dict:
    deadline = time.monotonic() + timeout_seconds
    last_state = {}
    while time.monotonic() < deadline:
        last_state = client.call("state", {})
        if last_state.get("in_play"):
            return last_state
        time.sleep(0.25)
    raise TimeoutError(f"client did not reach play; last_state={last_state}")


def write_observations(run_dir: Path, scenario_id: str, transcript: list[dict], result: str) -> None:
    observations = {
        "schema": "solaris.real_client_observations.v1",
        "client_gate": "agent-run-real-client",
        "quality_label": "stabilization",
        "result": result,
        "scenarios": [
            {
                "id": scenario_id,
                "result": result,
                "commands": transcript,
            }
        ],
    }
    (run_dir / "observations.json").write_text(json.dumps(observations, indent=2) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--secret", required=True)
    parser.add_argument("--run-dir", required=True)
    parser.add_argument("--scenario", default="m94-02b-rejected-block-resync")
    args = parser.parse_args()

    run_dir = Path(args.run_dir)
    client = AgentClient(args.host, args.port, args.secret)
    transcript = []
    for command, payload in [
        ("ping", {}),
        ("state", {}),
    ]:
        response = client.call(command, payload)
        transcript.append({"command": command, "payload": payload, "response": response})
    state = wait_play(client, 30.0)
    transcript.append({"command": "wait_play", "response": state})
    write_observations(run_dir, args.scenario, transcript, "passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 4: Run GREEN**

Run:

```bash
python3 -m unittest tools.tests.test_real_client_agent_driver -v
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/real-client-agent-driver.py tools/tests/test_real_client_agent_driver.py
git commit -m "test: add real-client agent driver"
```

## Task 7: Runner And Manifest Integration

**Files:**
- Modify: `tools/run-real-client-regression.sh`
- Modify: `docs/real-client-regression/README.md`
- Modify: `docs/real-client-regression/manifests/m94-regression-pack.json`
- Modify: `crates/mc-test-harness/tests/real_client_manifest.rs`

- [ ] **Step 1: Write failing runner/manifest guard**

Add to `approved_real_client_runner_is_fail_closed` in `crates/mc-test-harness/tests/real_client_manifest.rs`:

```rust
    assert!(
        runner.contains("SOLARIS_REAL_CLIENT_AGENT_PORT")
            && runner.contains("SOLARIS_REAL_CLIENT_AGENT_SECRET")
            && runner.contains("tools/real-client-agent-driver.py"),
        "runner must expose the invasive client-agent bridge hook"
    );
```

Add to `m94_real_client_manifest_covers_required_regression_rows`:

```rust
    assert_eq!(
        runner["agent_driver"].as_str(),
        Some("tools/real-client-agent-driver.py"),
        "M94 pack must name the invasive client-agent driver"
    );
```

- [ ] **Step 2: Run RED**

Run:

```bash
cargo test -p mc-test-harness --test real_client_manifest -- --nocapture
```

Expected: FAIL because the runner and manifest do not name the agent hook yet.

- [ ] **Step 3: Wire runner environment**

In `tools/run-real-client-regression.sh`, add near the existing env variables:

```bash
AGENT_PORT="${SOLARIS_REAL_CLIENT_AGENT_PORT:-39094}"
AGENT_SECRET="${SOLARIS_REAL_CLIENT_AGENT_SECRET:-}"
AGENT_DRIVER="${SOLARIS_REAL_CLIENT_AGENT_DRIVER:-$REPO_ROOT/tools/real-client-agent-driver.py}"
```

In `usage()`, add:

```text
  SOLARIS_REAL_CLIENT_AGENT_PORT     Loopback port opened by the in-client agent. Defaults to 39094.
  SOLARIS_REAL_CLIENT_AGENT_SECRET   Shared secret passed to the in-client agent and driver.
  SOLARIS_REAL_CLIENT_AGENT_DRIVER   Driver path. Defaults to tools/real-client-agent-driver.py.
```

In `write_run_templates()`, add:

```bash
    printf 'client_agent_port=%s\n' "$AGENT_PORT"
    if [[ -n "$AGENT_SECRET" ]]; then
      printf 'client_agent_secret=redacted\n'
      printf 'client_agent_secret_sha256=%s\n' "$(printf '%s' "$AGENT_SECRET" | sha256sum | cut -d ' ' -f 1)"
    else
      printf 'client_agent_secret=UNSET_PREPARED_OWNER_RUN\n'
    fi
    printf 'client_agent_driver=%s\n' "$AGENT_DRIVER"
```

After the client command exits in `--run`, add:

```bash
  if [[ -n "$AGENT_SECRET" && -x "$AGENT_DRIVER" ]]; then
    python3 "$AGENT_DRIVER" \
      --port "$AGENT_PORT" \
      --secret "$AGENT_SECRET" \
      --run-dir "$run_dir" \
      --scenario "m94-02b-rejected-block-resync" >> "$run_dir/automation-driver.txt" 2>&1 || true
  fi
```

This is intentionally non-green on failure: `--validate-run` still rejects prepared or missing observations.

- [ ] **Step 4: Update manifest and README**

In `docs/real-client-regression/manifests/m94-regression-pack.json`, add under `automation_runner`:

```json
"agent_driver": "tools/real-client-agent-driver.py",
"agent_port_env": "SOLARIS_REAL_CLIENT_AGENT_PORT",
"agent_secret_env": "SOLARIS_REAL_CLIENT_AGENT_SECRET",
```

In `docs/real-client-regression/README.md`, add:

```markdown
## In-Client Agent Driver

The invasive real-client path uses a Solaris-owned client mod that opens a
loopback-only bridge inside the real Minecraft client process. The approved
driver is `tools/real-client-agent-driver.py`. A run is still non-green until
`observations.json` records `"client_gate": "agent-run-real-client"` and
`tools/run-real-client-regression.sh --validate-run "$RUN_DIR"` passes.
```

- [ ] **Step 5: Run GREEN**

Run:

```bash
cargo test -p mc-test-harness --test real_client_manifest -- --nocapture
bash -n tools/run-real-client-regression.sh
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add tools/run-real-client-regression.sh \
  docs/real-client-regression/README.md \
  docs/real-client-regression/manifests/m94-regression-pack.json \
  crates/mc-test-harness/tests/real_client_manifest.rs
git commit -m "feat: wire real-client agent driver"
```

## Task 8: First End-To-End Bridge Smoke

**Files:**
- Modify only if Task 5 or Task 6 exposed a concrete compile/runtime gap.

- [ ] **Step 1: Run pure bridge and driver tests**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :bridge-core:test --no-daemon
cd ../..
python3 -m unittest tools.tests.test_real_client_agent_driver -v
```

Expected: PASS.

- [ ] **Step 2: Build the client-agent jar**

Run:

```bash
cd client-mod/solaris-client-agent
./gradlew :fabric-agent:remapJar --no-daemon
```

Expected: PASS if Fabric/Mojang metadata for `26.1.2` is available. If this fails because the version cannot be resolved, stop the client-mod slice with `blocked-client-tooling` and keep the pure bridge/runner work as stabilization scaffolding only.

- [ ] **Step 3: Run repository gates for touched areas**

Run:

```bash
cargo test -p mc-test-harness --test real_client_manifest -- --nocapture
cargo run -p xtask -- code-health
cargo fmt --all -- --check
git diff --check
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add client-mod tools docs crates/mc-test-harness/tests/real_client_manifest.rs
git commit -m "test: validate real-client agent scaffold"
```

## Task 9: Real Client Run Checkpoint

**Files:**
- Local-only artifacts under `.analysis/real-client-runs/`

- [ ] **Step 1: Prepare a run command**

Use a PrismLauncher/vanilla launch command from `SOLARIS_REAL_CLIENT_COMMAND`
that loads the built `solaris-client-agent` jar. The command must pass these JVM
properties:

```bash
-Dsolaris.clientAgent.port=39094
-Dsolaris.clientAgent.secret=$SOLARIS_REAL_CLIENT_AGENT_SECRET
-Dsolaris.clientAgent.runDir=$SOLARIS_REAL_CLIENT_RUN_DIR
```

- [ ] **Step 2: Run the real-client runner**

Run:

```bash
export SOLARIS_REAL_CLIENT_COMMAND="${SOLARIS_REAL_CLIENT_COMMAND:?set real PrismLauncher command that loads the agent jar}"
export SOLARIS_REAL_CLIENT_AGENT_SECRET="${SOLARIS_REAL_CLIENT_AGENT_SECRET:-$(openssl rand -hex 16)}"
SOLARIS_REAL_CLIENT_KIND=prism-launcher \
SOLARIS_REAL_CLIENT_AGENT_PORT=39094 \
SOLARIS_REAL_CLIENT_AGENT_SECRET="$SOLARIS_REAL_CLIENT_AGENT_SECRET" \
SOLARIS_REAL_CLIENT_COMMAND="$SOLARIS_REAL_CLIENT_COMMAND" \
bash tools/run-real-client-regression.sh --run
```

Expected: The client launches, the bridge driver writes `observations.json`, and the runner prints the local artifact path. If the GUI/client cannot launch in this environment, report prepared-only or owner-run instructions; do not mark Q2 improved.

- [ ] **Step 3: Validate artifacts**

Run:

```bash
RUN_DIR="$(find .analysis/real-client-runs -mindepth 1 -maxdepth 1 -type d | sort | tail -n 1)"
bash tools/run-real-client-regression.sh --validate-run "$RUN_DIR"
```

Expected: PASS only if `observations.json` has `"client_gate": "agent-run-real-client"` and a non-prepared result. If validation fails, keep the run as debugging evidence only.

- [ ] **Step 4: Commit only tracked docs/tests if evidence changes claims**

If and only if a real client run exists and validates, update `docs/VALIDATION_LEDGER.md` and `docs/REPLACEMENT_READINESS.md` with the exact run directory, scenario id, and observed pass/fail. Then run:

```bash
cargo run -p mc-test-harness --bin coverage-audit -- docs/VALIDATION_LEDGER.md
cargo test -p mc-test-harness --test real_client_manifest -- --nocapture
```

Expected: PASS. Coverage may still be 0.00% if the ledger row remains partial.

## Self-Review

- Spec coverage: The plan covers bridge transport, JSON protocol, client-thread command executor, artifacts, runner integration, and the first M94 scenario hook. MCP is deliberately excluded until the CLI/driver path works.
- Placeholder scan: The plan contains no `TBD`, no unnamed files, and no “implement later” steps. The only blocked path is explicit: unavailable Fabric/Mojang metadata for `26.1.2`.
- Type consistency: `BridgeRequest`, `BridgeResponse`, `BridgeError`, `BridgeCodec`, `AgentHttpBridge`, `CommandRegistry`, `ClientTaskExecutor`, `ClientFacade`, `ClientSnapshot`, and `ClientCommands` are introduced before use.
- Validation scope: Pure bridge and runner tests can pass without a GUI client. Real-client evidence still requires a real PrismLauncher/vanilla client run and remains non-green until `--validate-run` passes.
