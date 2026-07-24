plugins {
    java
    id("net.fabricmc.fabric-loom") version "1.17.11" apply false
    id("net.neoforged.moddev") version "2.0.141" apply false
    id("net.minecraftforge.gradle") version "7.0.17" apply false
}

allprojects {
    group = "dev.solaris"
    version = "0.1.0"
}

val loaderClientMcpToken = providers.environmentVariable("SOLARIS_CLIENT_MCP_TOKEN")
    .orElse(providers.gradleProperty("solaris.clientMcp.token"))
    .orElse("")
val loaderClientMcpPort = providers.environmentVariable("SOLARIS_CLIENT_MCP_PORT")
    .orElse(providers.gradleProperty("solaris.clientMcp.port"))
    .orElse("39095")
val loaderClientMcpGameDir = providers.gradleProperty("solaris.clientMcp.gameDir")
    .orElse(rootProject.file("../../.analysis/minecraft-loader-mcp").absolutePath)
val loaderClientMcpUsername = providers.gradleProperty("solaris.clientMcp.username")
    .orElse("SolarisLoader")

tasks.register("validateLoaderClientMcpRunProperties") {
    group = "verification"
    description = "Validates per-run Loader MCP client credentials and isolation inputs."
    doLast {
        if (loaderClientMcpToken.get().isBlank()) {
            throw GradleException(
                "Missing SOLARIS_CLIENT_MCP_TOKEN or -Psolaris.clientMcp.token for runClientMcp."
            )
        }
        val port = loaderClientMcpPort.get().toIntOrNull()
        if (port == null || port !in 1..65535) {
            throw GradleException(
                "Invalid MCP port '${loaderClientMcpPort.get()}'. Use SOLARIS_CLIENT_MCP_PORT=1..65535."
            )
        }
        val username = loaderClientMcpUsername.get()
        if (!username.matches(Regex("[A-Za-z0-9_]{1,16}"))) {
            throw GradleException(
                "Invalid -Psolaris.clientMcp.username '$username'. Use 1..16 ASCII letters, digits, or underscores."
            )
        }
        val gameDir = file(loaderClientMcpGameDir.get())
        if (!gameDir.isAbsolute) {
            throw GradleException(
                "Invalid -Psolaris.clientMcp.gameDir '${loaderClientMcpGameDir.get()}'. Use an absolute path."
            )
        }
    }
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
