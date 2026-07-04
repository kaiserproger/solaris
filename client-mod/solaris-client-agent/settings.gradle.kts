pluginManagement {
    repositories {
        maven("https://maven.fabricmc.net/")
        maven("https://maven.neoforged.net/releases/")
        maven("https://libraries.minecraft.net/")
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        maven("https://maven.fabricmc.net/")
        maven("https://maven.neoforged.net/releases/")
        maven("https://libraries.minecraft.net/")
        mavenCentral()
    }
}

rootProject.name = "solaris-client-agent"

include("bridge-core")
include("fabric-agent")
include("java-agent")
