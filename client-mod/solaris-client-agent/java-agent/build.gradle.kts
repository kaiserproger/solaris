plugins {
    java
}

val clientJar = rootProject.file("../../.analysis/client-automation/versions/26.1.2/client.jar")

dependencies {
    implementation(project(":bridge-core"))
    compileOnly(files(clientJar))
    compileOnly("com.mojang:brigadier:1.3.10")
    compileOnly("com.mojang:datafixerupper:9.0.19")
    compileOnly("io.netty:netty-buffer:4.2.7.Final")
    compileOnly("io.netty:netty-transport:4.2.7.Final")
    compileOnly("it.unimi.dsi:fastutil:8.5.18")
    compileOnly("org.jspecify:jspecify:1.0.0")
    testImplementation("org.junit.jupiter:junit-jupiter-api:5.10.2")
    testRuntimeOnly("org.junit.jupiter:junit-jupiter-engine:5.10.2")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher:1.10.2")
}

tasks.jar {
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
    manifest {
        attributes(
            "Premain-Class" to "dev.solaris.agent.javaagent.SolarisClientAgent",
            "Agent-Class" to "dev.solaris.agent.javaagent.SolarisClientAgent",
            "Can-Redefine-Classes" to "false",
            "Can-Retransform-Classes" to "false"
        )
    }
    from({
        configurations.runtimeClasspath.get().map { dependency ->
            if (dependency.isDirectory) dependency else zipTree(dependency)
        }
    })
}
