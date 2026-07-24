pluginManagement {
    repositories {
        maven("https://maven.fabricmc.net/")
        maven("https://maven.neoforged.net/releases/")
        maven("https://maven.minecraftforge.net")
        maven("https://libraries.minecraft.net/")
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        maven("https://maven.fabricmc.net/")
        maven("https://maven.neoforged.net/releases/")
        maven("https://maven.minecraftforge.net")
        maven("https://libraries.minecraft.net/")
        mavenCentral()
    }
}

rootProject.name = "solaris-client-agent"

include("bridge-core")
include("fabric-agent")
include("java-agent")
include("loader-core")
include("loader-fabric")
include("loader-neoforge")
include("loader-forge")
