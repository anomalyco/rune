#!/usr/bin/env bash
# Build (and optionally install + launch) an Android debug APK for a Rune example without Gradle.
# Usage: scripts/android-run.sh [--build-only] [--release] [--abi arm64-v8a|x86_64] [example-dir]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_ONLY=0
PROFILE="debug"
CARGO_PROFILE_ARGS=()
ABI="x86_64"
EXAMPLE_DIR="examples/android-hello"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --build-only) BUILD_ONLY=1 ;;
    --release) PROFILE="release"; CARGO_PROFILE_ARGS=(--release) ;;
    --abi) shift; ABI="$1" ;;
    --help|-h)
      sed -n '2,4p' "$0"
      exit 0
      ;;
    *) EXAMPLE_DIR="$1" ;;
  esac
  shift
done

case "$ABI" in
  arm64-v8a) TARGET="aarch64-linux-android" ;;
  x86_64) TARGET="x86_64-linux-android" ;;
  *) echo "unsupported ABI: $ABI (expected arm64-v8a or x86_64)" >&2; exit 2 ;;
esac

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [ -z "$SDK" ] || [ ! -d "$SDK" ]; then
  echo "ANDROID_HOME (or ANDROID_SDK_ROOT) is not set" >&2
  exit 1
fi

NDK="${ANDROID_NDK_LATEST_HOME:-${ANDROID_NDK_HOME:-}}"
if [ -z "$NDK" ] && [ -d "$SDK/ndk" ]; then
  NDK="$(ls -1d "$SDK"/ndk/* 2>/dev/null | sort -V | tail -n 1)"
fi
if [ -z "$NDK" ] || [ ! -d "$NDK" ]; then
  echo "Android NDK not found (set ANDROID_NDK_LATEST_HOME or ANDROID_NDK_HOME)" >&2
  exit 1
fi

PLATFORM_DIR="$(ls -1d "$SDK"/platforms/android-* 2>/dev/null | sort -V | tail -n 1)"
BUILD_TOOLS="$(ls -1d "$SDK"/build-tools/* 2>/dev/null | sort -V | tail -n 1)"
ANDROID_JAR="$PLATFORM_DIR/android.jar"
if [ ! -f "$ANDROID_JAR" ] || [ ! -x "$BUILD_TOOLS/aapt2" ] || [ ! -x "$BUILD_TOOLS/d8" ]; then
  echo "Android SDK platform or build-tools (aapt2, d8) missing under $SDK" >&2
  exit 1
fi

HOST_OS="$(uname -s)"
case "$HOST_OS" in
  Linux) PREBUILT="linux-x86_64" ;;
  Darwin) PREBUILT="darwin-x86_64" ;;
  *) echo "unsupported host OS for scripts/android-run.sh: $HOST_OS" >&2; exit 1 ;;
esac

NDK_BIN="$NDK/toolchains/llvm/prebuilt/$PREBUILT/bin"
CC_PATH="$(ls -1 "$NDK_BIN/$TARGET"*-clang 2>/dev/null | sort -V | tail -n 1)"
AR_PATH="$NDK_BIN/llvm-ar"
if [ -z "$CC_PATH" ] || [ ! -x "$CC_PATH" ]; then
  echo "Android NDK clang not found for $TARGET in $NDK_BIN" >&2
  exit 1
fi

TARGET_ENV="${TARGET//-/_}"
TARGET_UPPER="$(printf '%s' "$TARGET_ENV" | tr '[:lower:]' '[:upper:]')"
export "CC_${TARGET_ENV}=$CC_PATH"
export "AR_${TARGET_ENV}=$AR_PATH"
export "CARGO_TARGET_${TARGET_UPPER}_LINKER=$CC_PATH"

PKG="$(awk -F'"' '/^name[[:space:]]*=/{print $2; exit}' "$ROOT/$EXAMPLE_DIR/Cargo.toml")"
LIB_STEM="${PKG//-/_}"

echo "Building $PKG ($TARGET, $PROFILE)..."
cargo build --manifest-path "$ROOT/Cargo.toml" --target "$TARGET" -p "$PKG" "${CARGO_PROFILE_ARGS[@]}"

SO_PATH="$ROOT/target/$TARGET/$PROFILE/lib${LIB_STEM}.so"
if [ ! -f "$SO_PATH" ]; then
  echo "expected shared library not found: $SO_PATH" >&2
  exit 1
fi

python3 "$ROOT/scripts/check_android_so.py" \
  --so "$SO_PATH" \
  --clang "$CC_PATH" \
  --nm "$NDK_BIN/llvm-nm" \
  --readelf "$NDK_BIN/llvm-readelf" \
  --require-symbol JNI_OnLoad

OUT="$ROOT/target/android/$PKG/$ABI"
rm -rf "$OUT"
mkdir -p "$OUT/classes" "$OUT/dex" "$OUT/apk/lib/$ABI"

echo "Compiling Kotlin host (dev.rune)..."
kotlinc   -cp "$ANDROID_JAR"   -jvm-target 11   -d "$OUT/classes.jar"   "$ROOT"/crates/rune/android/src/main/java/dev/rune/*.kt

KOTLIN_STDLIB=""
KOTLINC_BIN="$(command -v kotlinc)"
KOTLIN_HOME="$(cd "$(dirname "$KOTLINC_BIN")/.." && pwd)"
if [ -f "$KOTLIN_HOME/lib/kotlin-stdlib.jar" ]; then
  KOTLIN_STDLIB="$KOTLIN_HOME/lib/kotlin-stdlib.jar"
elif [ -f "$KOTLIN_HOME/libexec/lib/kotlin-stdlib.jar" ]; then
  KOTLIN_STDLIB="$KOTLIN_HOME/libexec/lib/kotlin-stdlib.jar"
fi

D8_INPUTS=("$OUT/classes.jar")
if [ -n "$KOTLIN_STDLIB" ]; then
  D8_INPUTS+=("$KOTLIN_STDLIB")
fi

"$BUILD_TOOLS/d8"   --lib "$ANDROID_JAR"   --min-api 26   --output "$OUT/dex"   "${D8_INPUTS[@]}"

echo "Packaging APK..."
"$BUILD_TOOLS/aapt2" link   -I "$ANDROID_JAR"   --manifest "$ROOT/$EXAMPLE_DIR/AndroidManifest.xml"   -o "$OUT/unaligned.apk"

cp "$OUT/dex/classes.dex" "$OUT/apk/classes.dex"
cp "$SO_PATH" "$OUT/apk/lib/$ABI/lib${LIB_STEM}.so"

(cd "$OUT/apk" && zip -q -r "$OUT/unaligned.apk" classes.dex "lib/$ABI/lib${LIB_STEM}.so")

"$BUILD_TOOLS/zipalign" -f -p 4 "$OUT/unaligned.apk" "$OUT/aligned.apk"

KEYSTORE="$OUT/debug.keystore"
keytool -genkeypair -noprompt   -keystore "$KEYSTORE"   -storepass android -keypass android   -alias androiddebugkey   -keyalg RSA -keysize 2048 -validity 10000   -dname "CN=Rune Debug,O=Rune,C=US" >/dev/null 2>&1

APK="$OUT/${PKG}-${ABI}.apk"
"$BUILD_TOOLS/apksigner" sign   --ks "$KEYSTORE"   --ks-pass pass:android   --key-pass pass:android   --out "$APK"   "$OUT/aligned.apk"

echo "Built APK: $APK"
if [ "$BUILD_ONLY" -eq 1 ]; then
  exit 0
fi

APP_ID="$(awk -F'"' '/package=/{print $2; exit}' "$ROOT/$EXAMPLE_DIR/AndroidManifest.xml")"
echo "Installing and launching $APP_ID..."
adb install -r "$APK"
adb shell am start -n "$APP_ID/dev.rune.RuneActivity"
