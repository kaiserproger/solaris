plugins {
    java
    id("fabric-loom") version "1.17.11" apply false
    id("net.neoforged.moddev") version "2.0.141" apply false
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
