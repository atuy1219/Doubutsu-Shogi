plugins {
    id("com.android.application")
}

android {
    namespace = "com.atuy.dobutsueval"
    compileSdk = 36
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "com.atuy.dobutsueval"
        minSdk = 28
        targetSdk = 36
        versionCode = 25
        versionName = "0.8.0"
        ndk { abiFilters += listOf("arm64-v8a") }
        externalNativeBuild {
            cmake { cppFlags += listOf("-std=c++20", "-Wall", "-Wextra", "-Werror=return-type") }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    externalNativeBuild {
        cmake { path = file("src/main/cpp/CMakeLists.txt"); version = "3.22.1" }
    }
    androidResources {
        noCompress += "tablebase"
    }
    packaging {
        jniLibs { useLegacyPackaging = true }
    }
}

dependencies {
    compileOnly("io.github.libxposed:api:102.0.0")
}

val verifyBundledTablebase by tasks.registering {
    val tablebase = layout.projectDirectory.file("src/main/assets/dobutsu.tablebase").asFile
    doLast {
        check(tablebase.isFile && tablebase.length() > 20_000_000L) {
            "The complete verified tablebase is required: app/src/main/assets/dobutsu.ref.gz"
        }
    }
}

tasks.named("preBuild").configure {
    dependsOn(verifyBundledTablebase)
}
