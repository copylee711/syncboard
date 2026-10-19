import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// Release signing lives outside the repo (default ~/.syncboard/keystore.properties, or the
// file named by SYNCBOARD_SIGNING). Without it, release builds fall back to the debug key.
val releaseSigning = Properties().apply {
    val f = file(System.getenv("SYNCBOARD_SIGNING") ?: "${System.getProperty("user.home")}/.syncboard/keystore.properties")
    if (f.isFile) f.inputStream().use { load(it) }
}
// CI must never publish a debug-signed APK: installed copies could not be updated with it.
if (releaseSigning.isEmpty() && System.getenv("SYNCBOARD_REQUIRE_SIGNING") == "1") {
    throw GradleException("Release signing is required but no keystore.properties was found")
}

android {
    namespace = "app.syncboard"
    compileSdk {
        version = release(37)
    }

    defaultConfig {
        applicationId = "app.syncboard"
        minSdk = 29
        targetSdk = 36
        versionCode = 12
        versionName = "0.4.0"
    }

    signingConfigs {
        if (releaseSigning.isNotEmpty()) {
            create("release") {
                storeFile = file(releaseSigning.getProperty("storeFile"))
                storePassword = releaseSigning.getProperty("storePassword")
                keyAlias = releaseSigning.getProperty("keyAlias")
                keyPassword = releaseSigning.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            optimization {
                enable = true
            }
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
    }
    packaging {
        resources.excludes += setOf("META-INF/**", "kotlin/**", "DebugProbesKt.bin")
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.camera.camera2)
    implementation(libs.camera.lifecycle)
    implementation(libs.camera.view)
    implementation(libs.zxing.core)
    debugImplementation(libs.androidx.compose.ui.tooling)
    compileOnly(project(":hidden-stubs"))
    testImplementation(libs.junit)
    testImplementation(libs.org.json)
}
