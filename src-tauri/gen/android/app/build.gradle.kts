import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// Load the private key and credentials from outside the checkout.
val signingPropertiesPath = providers.environmentVariable("KIYORA_ANDROID_SIGNING_PROPERTIES")
val signingPropertiesFile = file(signingPropertiesPath.orElse(
    "${System.getProperty("user.home")}/.local/share/kiyora/android-signing/keystore.properties"
).get())
check(!signingPropertiesPath.isPresent || signingPropertiesFile.isFile) {
    "KIYORA_ANDROID_SIGNING_PROPERTIES must point to an existing keystore.properties file"
}
val releaseSigningProperties = Properties().apply {
    if (signingPropertiesFile.isFile) {
        signingPropertiesFile.inputStream().use { load(it) }
    }
}
fun signingProperty(name: String): String =
    requireNotNull(releaseSigningProperties.getProperty(name)?.takeIf { it.isNotBlank() }) {
        "Missing Android release signing property: $name"
    }

android {
    compileSdk = 36
    buildToolsVersion = "36.0.0"
    namespace = "io.github.arkfarin.kiyora"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "io.github.arkfarin.kiyora"
        minSdk = 26
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        testInstrumentationRunnerArguments["waitForActivitiesToComplete"] = "false"
    }
    signingConfigs {
        if (signingPropertiesFile.isFile) {
            create("release") {
                storeFile = signingPropertiesFile.parentFile.resolve(signingProperty("storeFile"))
                storePassword = signingProperty("storePassword")
                keyAlias = signingProperty("keyAlias")
                keyPassword = signingProperty("keyPassword")
                storeType = releaseSigningProperties.getProperty("storeType", "PKCS12")
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            signingConfigs.findByName("release")?.let { signingConfig = it }
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
    sourceSets.getByName("main") {
        jniLibs.srcDir(file("../../../target/native/android-jni"))
        assets.srcDir(file("../../../target/native/android-assets"))
    }
    packaging.jniLibs {
        // The Rust adapter opens the package manager's nativeLibraryDir path.
        // Explicit extraction keeps that path valid on every supported Android.
        useLegacyPackaging = true
        keepDebugSymbols.add("**/libtdjson.so")
    }
}

val nativeAbis = providers.gradleProperty("abiList").orElse("arm64-v8a").get().split(',')
tasks.named("preBuild") {
    val libraries = nativeAbis.map { abi -> abi to file("../../../target/native/android-jni/$abi/libtdjson.so") }
    inputs.files(libraries.map { it.second })
    doFirst {
        for ((abi, library) in libraries) {
            check(library.isFile) {
                "Run scripts/prepare-android-native.sh with NDK_HOME and KIYORA_ANDROID_ABI=$abi before building the app APK"
            }
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
