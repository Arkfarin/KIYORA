plugins {
    id("com.android.library") apply false
    id("org.jetbrains.kotlin.android") apply false
}
val output = file("../../target/android-plugin")
layout.buildDirectory.set(output.resolve("root"))
subprojects {
    layout.buildDirectory.set(output.resolve(name))
    plugins.withId("com.android.library") {
        extensions.configure<com.android.build.gradle.LibraryExtension> {
            buildToolsVersion = "36.0.0"
        }
    }
}
