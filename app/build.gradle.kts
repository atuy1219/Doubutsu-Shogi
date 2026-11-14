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
        versionCode = 24
        versionName = "0.7.2"
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
    packaging {
        jniLibs { useLegacyPackaging = true }
    }
}

dependencies {
    compileOnly("io.github.libxposed:api:102.0.0")
}
