pluginManagement {
    repositories { google(); mavenCentral(); gradlePluginPortal() }
    plugins {
        id("com.android.library") version "8.10.1"
        id("org.jetbrains.kotlin.android") version "2.2.0"
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories { google(); mavenCentral() }
}
rootProject.name = "kiyora-native-check"
include(":kiyora-native", ":tauri-android")
project(":kiyora-native").projectDir = file("../../crates/tauri-plugin-kiyora-native/android")
project(":tauri-android").projectDir = file(requireNotNull(System.getenv("KIYORA_TAURI_ANDROID")))
