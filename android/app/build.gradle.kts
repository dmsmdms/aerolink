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
        versionCode = 1
        versionName = "0.1.0"
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

    testOptions {
        unitTests.all {
            // Shared USB protocol reference frames (docs/03-usb-link.md)
            it.systemProperty("aerolink.vectors", rootProject.file("../protocol/vectors.txt").absolutePath)
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

    implementation("androidx.activity:activity-compose:1.8.2")
    implementation(platform("androidx.compose:compose-bom:2023.08.00"))

    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")

    implementation("androidx.compose.material3:material3")

    testImplementation("junit:junit:4.13.2")
}
