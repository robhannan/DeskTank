plugins {
    alias(libs.plugins.android.application)
}

// The ABIs libdesktank_android.so is built for (SPEC §3.6): 64- and 32-bit
// ARM devices and the x86_64 emulator. `-Pdesktank.abis=arm64-v8a` builds
// fewer while iterating.
val abis = providers.gradleProperty("desktank.abis")
    .map { it.split(',').map(String::trim).filter(String::isNotEmpty) }
    .getOrElse(listOf("arm64-v8a", "armeabi-v7a", "x86_64"))
val pinnedNdk: String = libs.versions.ndk.get()
// Android 8 (SPEC §3.6; the A0 findings in docs/PROGRESS.md). The Rust
// library is linked against the same API level.
val minApi = 26

android {
    namespace = "io.github.robhannan.desktank"
    compileSdk = 36
    ndkVersion = pinnedNdk

    defaultConfig {
        applicationId = "io.github.robhannan.desktank"
        minSdk = minApi
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk { abiFilters += abis }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            jniLibs.directories.add(layout.buildDirectory.dir("rustJniLibs").get().asFile.path)
        }
    }
}

// Builds libdesktank_android.so for each ABI with cargo-ndk (release profile:
// a debug wgpu is too large and slow to judge the spike by), into the
// directory packaged above. Cargo decides what is up to date.
val cargoNdk = tasks.register<Exec>("cargoNdk") {
    val out = layout.buildDirectory.dir("rustJniLibs")
    workingDir = rootDir.parentFile
    // The pinned NDK, so cargo-ndk and AGP use the same one.
    val ndkDir = androidComponents.sdkComponents.sdkDirectory.map { it.dir("ndk/$pinnedNdk") }
    doFirst {
        val path = ndkDir.get().asFile.path
        environment("ANDROID_NDK_HOME", path)
        environment("ANDROID_NDK_ROOT", path)
    }
    commandLine(
        buildList {
            addAll(listOf("cargo", "ndk", "--platform", minApi.toString()))
            abis.forEach { addAll(listOf("-t", it)) }
            addAll(listOf("-o", out.get().asFile.path))
            addAll(listOf("build", "--release", "--locked", "-p", "desktank-android"))
        },
    )
    outputs.dir(out)
    outputs.upToDateWhen { false }
}

tasks.named("preBuild") { dependsOn(cargoNdk) }
