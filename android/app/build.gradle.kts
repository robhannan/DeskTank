plugins {
    alias(libs.plugins.android.application)
}

// The ABIs libdesktank_android.so is built for (SPEC §3.6): 64- and 32-bit
// ARM devices and the x86_64 emulator. `-Pdesktank.abis=arm64-v8a` builds
// fewer while iterating.
val abis = providers.gradleProperty("desktank.abis")
    .map { it.split(',').map(String::trim).filter(String::isNotEmpty) }
    .getOrElse(listOf("arm64-v8a", "armeabi-v7a", "x86_64"))

// The build's commit, by the rules of scripts/build-web.ts and the
// desktank-build-info crate: GITHUB_SHA (first 7 characters), else
// `git rev-parse --short`, `-dirty` for uncommitted changes, `unknown` without
// git. It lands in versionName, which Android's App info shows.
fun git(vararg args: String): String = providers.exec {
    workingDir = rootDir
    commandLine("git", *args)
    isIgnoreExitValue = true
}.standardOutput.asText.get().trim()

val commit: String = providers.environmentVariable("GITHUB_SHA").orNull
    ?.trim()?.take(7)?.takeIf { it.isNotEmpty() }
    ?: git("rev-parse", "--short=7", "HEAD").ifEmpty { null }
    ?: "unknown"
val dirty = commit != "unknown" && git("status", "--porcelain").isNotEmpty()
val buildLabel = if (dirty) "$commit-dirty" else commit
val pinnedNdk: String = libs.versions.ndk.get()
// Android 8 (docs/spec/platforms.md §3.6; A0 findings in docs/notes/android.md). The Rust
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
        versionName = "0.1.0-$buildLabel"
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
