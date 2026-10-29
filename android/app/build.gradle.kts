plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.aerolink.robot"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.aerolink.robot"
        minSdk = 23 // Android 6.0
        targetSdk = 34
        // Robots update only to a higher versionCode (ROB-008); -Paerolink.versionCode=… for bench tests.
        versionCode = (project.findProperty("aerolink.versionCode") as String?)?.toInt() ?: 1
        versionName = "0.1.0"
        // Only our languages: drops the translations of AndroidX/Material (docs: EN + RU UI).
        resourceConfigurations += listOf("en", "ru")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    composeOptions {
        kotlinCompilerExtensionVersion = "1.5.1"
    }

    // One APK per ABI: WebRTC's native library is ~80 % of the app (4–6 MB compressed per ABI). The robot phone
    // (SM-J710F) needs armeabi-v7a, the Pixel 7 bench phone arm64-v8a only (it cannot run 32-bit code).
    splits {
        abi {
            isEnable = true
            reset()
            include("armeabi-v7a", "arm64-v8a")
            isUniversalApk = false
        }
    }

    // ROB-009: the RP2040 firmware built by `make rp2040` (build/android-assets/firmware/aerolink-rp2040.bin)
    // ships in the APK; the app keeps the robot base on exactly this image.
    sourceSets {
        getByName("main") { assets.srcDir(rootProject.file("../build/android-assets")) }
    }

    packaging {
        // Native libs compressed in the APK and extracted on install: the APK is ~40 % smaller, and Android 15+
        // devices with 16 KB pages do not need the uncompressed .so to be 16 KB zip-aligned (AGP 8.2 does not).
        jniLibs { useLegacyPackaging = true }
    }

    testOptions {
        // android.util.Log and friends are no-ops in JVM tests.
        unitTests.isReturnDefaultValues = true
        unitTests.all {
            // Shared USB protocol reference frames (docs/03-usb-link.md)
            it.systemProperty("aerolink.vectors", rootProject.file("../protocol/vectors.txt").absolutePath)
            // BackendInteropTest, run by tools/e2e_console.sh against a live backend (skipped when empty).
            for (key in listOf("aerolink.backend", "aerolink.robotToken", "aerolink.origin")) {
                it.systemProperty(key, project.findProperty(key) ?: "")
            }
        }
    }

    signingConfigs {
        create("release") {
            val storeFileProp = project.findProperty("AEROLINK_STORE_FILE") as String?
            if (storeFileProp != null) {
                storeFile = file(storeFileProp)
                storePassword = project.property("AEROLINK_STORE_PASSWORD") as String
                keyAlias = project.property("AEROLINK_KEY_ALIAS") as String
                keyPassword = project.property("AEROLINK_KEY_PASSWORD") as String
            }
        }
    }

    buildTypes {
        getByName("release") {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            signingConfig = if (project.hasProperty("AEROLINK_STORE_FILE")) {
                signingConfigs.getByName("release")
            } else {
                logger.warn("AEROLINK_STORE_FILE is not set: release APK is signed with the debug key")
                signingConfigs.getByName("debug")
            }
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.12.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.7.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.7.1")

    // Operator link (docs/08-backend.md): signaling WebSocket and peer-to-peer WebRTC.
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("io.github.webrtc-sdk:android:150.7871.04")

    implementation("androidx.activity:activity-compose:1.8.2")
    implementation(platform("androidx.compose:compose-bom:2023.08.00"))

    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")

    implementation("androidx.compose.material3:material3")

    testImplementation("junit:junit:4.13.2")
    // android.jar's org.json is a stub in JVM tests.
    testImplementation("org.json:json:20240303")
    testImplementation("com.squareup.okhttp3:mockwebserver:4.12.0")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.7.1")
}
