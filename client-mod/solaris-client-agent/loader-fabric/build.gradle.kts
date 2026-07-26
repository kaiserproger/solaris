plugins {
    java
    id("net.fabricmc.fabric-loom")
}

val minecraftVersion = providers.gradleProperty("minecraftVersion")
val fabricLoaderVersion = providers.gradleProperty("fabricLoaderVersion")
val fabricApiVersion = providers.gradleProperty("fabricApiVersion")
val clientMcpToken = providers.environmentVariable("SOLARIS_CLIENT_MCP_TOKEN")
    .orElse(providers.gradleProperty("solaris.clientMcp.token"))
    .orElse("")
val clientMcpPort = providers.environmentVariable("SOLARIS_CLIENT_MCP_PORT")
    .orElse(providers.gradleProperty("solaris.clientMcp.port"))
    .orElse("39095")
val clientMcpGameDir = providers.gradleProperty("solaris.clientMcp.gameDir")
    .orElse(rootProject.file("../../.analysis/minecraft-loader-mcp/fabric").absolutePath)
val clientMcpUsername = providers.gradleProperty("solaris.clientMcp.username").orElse("SolarisLoader")

loom {
    runs {
        create("clientMcp") {
            client()
        }
    }
}

dependencies {
    implementation(project(":loader-core"))
    minecraft("com.mojang:minecraft:${minecraftVersion.get()}")
    implementation("net.fabricmc:fabric-loader:${fabricLoaderVersion.get()}")
    implementation("net.fabricmc.fabric-api:fabric-api:${fabricApiVersion.get()}")
    include(project(":loader-core"))
    runtimeOnly(project(":java-agent"))
    testImplementation("org.junit.jupiter:junit-jupiter-api:5.10.2")
    testRuntimeOnly("org.junit.jupiter:junit-jupiter-engine:5.10.2")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher:1.10.2")
}

sourceSets {
    named("main") {
        java.srcDir(rootProject.file("loader-platform-common/src/main/java"))
    }
    named("test") {
        java.srcDir(rootProject.file("loader-platform-common/src/test/java"))
    }
}

tasks.named<JavaExec>("runClientMcp") {
    group = "verification"
    description = "Runs the Fabric Loader client with the embedded Solaris MCP server."
    dependsOn(rootProject.tasks.named("validateLoaderClientMcpRunProperties"))
    workingDir(file(clientMcpGameDir.get()))
    args("--username", clientMcpUsername.get())
    environment("SOLARIS_CLIENT_MCP_TOKEN", clientMcpToken.get())
    environment("SOLARIS_CLIENT_MCP_PORT", clientMcpPort.get())
    systemProperty(
        "solaris.loader.cacheDir",
        file(clientMcpGameDir.get()).resolve("solaris-loader-cache").absolutePath
    )
    notCompatibleWithConfigurationCache("The Loader MCP profile uses per-run credentials and game directories.")
}
