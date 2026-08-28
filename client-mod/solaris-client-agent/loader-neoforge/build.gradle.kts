plugins {
    java
    id("net.neoforged.moddev")
}

val neoVersion = providers.gradleProperty("neoVersion")
val clientMcpToken = providers.environmentVariable("SOLARIS_CLIENT_MCP_TOKEN")
    .orElse(providers.gradleProperty("solaris.clientMcp.token"))
    .orElse("")
val clientMcpPort = providers.environmentVariable("SOLARIS_CLIENT_MCP_PORT")
    .orElse(providers.gradleProperty("solaris.clientMcp.port"))
    .orElse("39095")
val clientMcpGameDir = providers.gradleProperty("solaris.clientMcp.gameDir")
    .orElse(rootProject.file("../../.analysis/minecraft-loader-mcp/neoforge").absolutePath)
val clientMcpUsername = providers.gradleProperty("solaris.clientMcp.username").orElse("SolarisLoader")
dependencies {
    implementation(project(":loader-core"))
    implementation(project(":bridge-core"))
    testImplementation("org.junit.jupiter:junit-jupiter-api:5.10.2")
    testRuntimeOnly("org.junit.jupiter:junit-jupiter-engine:5.10.2")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher:1.10.2")
}

sourceSets {
    named("main") {
        java.srcDir(rootProject.file("loader-platform-common/src/main/java"))
        java.srcDir(project(":java-agent").layout.projectDirectory.dir("src/main/java"))
    }
    named("test") {
        java.srcDir(rootProject.file("loader-platform-common/src/test/java"))
    }
}

neoForge {
    version = neoVersion.get()
    runs {
        create("clientMcp") {
            client()
            gameDirectory.set(file(clientMcpGameDir.get()))
            environment("SOLARIS_CLIENT_MCP_TOKEN", clientMcpToken.get())
            environment("SOLARIS_CLIENT_MCP_PORT", clientMcpPort.get())
            systemProperty(
                "solaris.loader.cacheDir",
                file(clientMcpGameDir.get()).resolve("solaris-loader-cache").absolutePath
            )
            programArgument("--username")
            programArgument(clientMcpUsername.get())
        }
    }
    mods {
        create("solarisLoader") {
            sourceSet(sourceSets.main.get())
        }
    }
    unitTest {
        enable()
        testedMod = mods["solarisLoader"]
    }
}

tasks.jar {
    dependsOn(project(":loader-core").tasks.named("classes"))
    from(project(":loader-core").layout.buildDirectory.dir("classes/java/main"))
}

tasks.named<JavaExec>("runClientMcp") {
    group = "verification"
    description = "Runs the NeoForge Loader client with the embedded Solaris MCP server."
    dependsOn(rootProject.tasks.named("validateLoaderClientMcpRunProperties"))
    notCompatibleWithConfigurationCache("The Loader MCP profile uses per-run credentials and game directories.")
}
