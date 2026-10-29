#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$PROJECT_ROOT/build"
ANDROID_DIR="$PROJECT_ROOT/android"

mkdir -p "$OUT_DIR"
cd "$ANDROID_DIR"

# local.properties is machine-specific and not in git
if [ ! -f local.properties ]; then
    SDK_DIR="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-/opt/android-sdk}}"
    echo "sdk.dir=$SDK_DIR" > local.properties
    echo "ℹ️ Created android/local.properties (sdk.dir=$SDK_DIR)"
fi

# release (default) or debug: debug allows plain http/ws to a LAN backend and offers the simulated robot base.
VARIANT="${ANDROID_VARIANT:-release}"
case "$VARIANT" in
    release) TASK=assembleRelease ;;
    debug) TASK=assembleDebug ;;
    *) echo "❌ ANDROID_VARIANT must be release or debug"; exit 1 ;;
esac

# ROB-009: the APK carries the RP2040 firmware (build/android-assets, written by `make rp2040`).
FIRMWARE="$OUT_DIR/android-assets/firmware/aerolink-rp2040.bin"
if [ ! -f "$FIRMWARE" ]; then
    if [ "$VARIANT" = release ]; then
        echo "❌ Error: $FIRMWARE not found, run 'make rp2040' first (release APKs update the robot base's firmware)."
        exit 1
    fi
    echo "⚠️ No firmware image ($FIRMWARE): this debug APK will not update robot bases."
fi

# ANDROID_VERSION_CODE overrides versionCode (bench tests of the update path, ROB-008).
GRADLE_ARGS=()
[ -n "$ANDROID_VERSION_CODE" ] && GRADLE_ARGS+=("-Paerolink.versionCode=$ANDROID_VERSION_CODE")

echo "🤖 Building Android APK ($VARIANT)..."
gradle "$TASK" --warning-mode all "${GRADLE_ARGS[@]}"

# One APK per ABI (app/build.gradle.kts "splits"): build/aerolink-<abi>.apk
rm -f "$OUT_DIR"/aerolink*.apk "$OUT_DIR/aerolink-release.json"
for ABI in armeabi-v7a arm64-v8a; do
    APK_SOURCE="$ANDROID_DIR/app/build/outputs/apk/$VARIANT/app-$ABI-$VARIANT.apk"
    if [ ! -f "$APK_SOURCE" ]; then
        echo "❌ Error: $APK_SOURCE not found!"
        exit 1
    fi
    cp "$APK_SOURCE" "$OUT_DIR/aerolink-$ABI.apk"
    echo "✅ Android APK ($VARIANT, $ABI, $(du -h "$APK_SOURCE" | cut -f1)) placed to build/aerolink-$ABI.apk"
done

# The release robots update to (ROB-008, docs/09-deployment.md "App updates"): version from Gradle's output
# metadata, size and SHA-256 of each APK.
python3 - "$ANDROID_DIR/app/build/outputs/apk/$VARIANT/output-metadata.json" "$OUT_DIR" <<'PY'
import hashlib, json, os, sys
meta, out = json.load(open(sys.argv[1])), sys.argv[2]
apks = {}
for e in meta["elements"]:
    abi = next(f["value"] for f in e["filters"] if f["filterType"] == "ABI")
    name = f"aerolink-{abi}.apk"
    data = open(os.path.join(out, name), "rb").read()
    apks[abi] = {"file": name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
e = meta["elements"][0]
release = {"version_code": e["versionCode"], "version_name": e["versionName"], "apks": apks}
json.dump(release, open(os.path.join(out, "aerolink-release.json"), "w"), indent=2)
print(f"✅ Release {e['versionName']} ({e['versionCode']}) described in build/aerolink-release.json")
PY
