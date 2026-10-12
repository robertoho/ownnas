import java.util.Properties
import javax.inject.Inject

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

/// The Cargo workspace that holds `crates/ownnas-ffi`.
val workspaceDir: File = rootDir.resolve("../..").canonicalFile
/// Android ABIs to build the Rust library for: phones and the emulator.
val rustAbis = listOf("arm64-v8a", "x86_64")
val ndkVersionPinned = "29.0.14206865"

android {
    namespace = "app.ownnas.android"
    compileSdk = 36
    ndkVersion = ndkVersionPinned

    defaultConfig {
        applicationId = "app.ownnas.android"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk { abiFilters += rustAbis }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
    }
}

kotlin {
    jvmToolchain(17)
}

/// Where cargo lives: `rust.bin` in local.properties, else the usual rustup locations. Android
/// Studio started from the Dock does not see the shell's PATH, so these are searched first.
val rustDirs: List<String> = run {
    val local = Properties()
    rootDir.resolve("local.properties").takeIf { it.exists() }?.inputStream()?.use(local::load)
    val home = System.getProperty("user.home")
    listOfNotNull(local.getProperty("rust.bin"), "/opt/homebrew/opt/rustup/bin", "/usr/local/opt/rustup/bin", "$home/.cargo/bin") +
        System.getenv("PATH").orEmpty().split(File.pathSeparator)
}
val rustPath: String = rustDirs.joinToString(File.pathSeparator)
val cargoExecutable: String = rustDirs.map { File(it, "cargo") }.firstOrNull { it.canExecute() }?.absolutePath ?: "cargo"

/// Builds `libownnas_ffi.so` for each ABI with cargo-ndk. Cargo decides what is stale.
abstract class CargoNdkTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations
    @get:Input abstract val release: Property<Boolean>
    @get:Input abstract val abis: ListProperty<String>
    @get:Input abstract val cargo: Property<String>
    @get:Input abstract val searchPath: Property<String>
    @get:Input abstract val ndkHome: Property<String>
    @get:Internal abstract val workspace: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun build() {
        val args = mutableListOf("ndk", "--platform", "26")
        abis.get().forEach { args += listOf("-t", it) }
        args += listOf("-o", outputDir.get().asFile.absolutePath, "build", "-p", "ownnas-ffi")
        if (release.get()) args += "--release"
        exec.exec {
            workingDir = workspace.get().asFile
            environment("PATH", searchPath.get())
            environment("ANDROID_NDK_HOME", ndkHome.get())
            commandLine(listOf(cargo.get()) + args)
        }
    }
}

/// Generates the Kotlin bindings (package `app.ownnas.core`) from a built library.
abstract class UniffiBindgenTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations
    @get:InputDirectory abstract val libraries: DirectoryProperty
    @get:Input abstract val cargo: Property<String>
    @get:Input abstract val searchPath: Property<String>
    @get:Internal abstract val workspace: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val library = libraries.get().asFile.walk().first { it.name == "libownnas_ffi.so" }
        outputDir.get().asFile.deleteRecursively()
        exec.exec {
            workingDir = workspace.get().asFile
            environment("PATH", searchPath.get())
            commandLine(
                cargo.get(), "run", "-q", "-p", "ownnas-ffi", "--features", "cli", "--bin", "uniffi-bindgen", "--",
                "generate", "--library", library.absolutePath, "--language", "kotlin", "--no-format",
                "--out-dir", outputDir.get().asFile.absolutePath,
            )
        }
    }
}

androidComponents {
    onVariants { variant ->
        val name = variant.name.replaceFirstChar(Char::uppercase)
        val cargoTask = tasks.register<CargoNdkTask>("cargoNdk$name") {
            release.set(variant.buildType == "release")
            abis.set(rustAbis)
            cargo.set(cargoExecutable)
            searchPath.set(rustPath)
            ndkHome.set(android.sdkDirectory.resolve("ndk/$ndkVersionPinned").absolutePath)
            workspace.set(workspaceDir)
            outputDir.set(layout.buildDirectory.dir("rust/${variant.name}/jniLibs"))
            outputs.upToDateWhen { false }
        }
        val bindgenTask = tasks.register<UniffiBindgenTask>("uniffiBindgen$name") {
            libraries.set(cargoTask.flatMap { it.outputDir })
            cargo.set(cargoExecutable)
            searchPath.set(rustPath)
            workspace.set(workspaceDir)
            outputDir.set(layout.buildDirectory.dir("generated/uniffi/${variant.name}"))
        }
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoTask, CargoNdkTask::outputDir)
        variant.sources.java?.addGeneratedSourceDirectory(bindgenTask, UniffiBindgenTask::outputDir)
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.extended)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.android)
    implementation("${libs.jna.get()}@aar")
    debugImplementation(libs.androidx.compose.ui.tooling)
}
