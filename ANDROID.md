# Android

Rune runs natively on Android without `winit`'s event loop at runtime: a small Kotlin host (`dev.rune.RuneActivity` and `dev.rune.RuneView`) owns an Android `SurfaceView` and `Choreographer`, and drives Rune's `LayerHost` directly over an `ANativeWindow` via JNI.

## Architecture (S1)

- **Rust host (`crates/rune/src/platform/android/`)**:
  - `machine.rs`: pure lifecycle state machine covering surface creation, resize/rotation, `surfaceDestroyed`/`surfaceCreated` trips, `onPause`/`onResume`, Choreographer scheduling deduplication, insets, dark mode, and Back navigation.
  - `host.rs`: JNI bridge bound in `JNI_OnLoad` via `RegisterNatives`, type-erasing `App<S>` into `LayerHost<S>` when `App::try_run` runs.
  - `logcat.rs`: forwards `stdout`, `stderr`, and Rust panics to Android logcat under tag `Rune`.
- **Embedded surface (`rune::embed::LayerHost`)**:
  - Accepts an `ANativeWindow *` via `LayerHost::from_native_window`.
  - Supports `LayerHost::detach_surface` and `LayerHost::attach_native_window` so the `Runtime<S>`, app state, and GPU device survive across Home/return surface destruction.
  - Uses `wgpu::Limits::downlevel_defaults()` automatically on adapters with 16 KiB uniform buffer limits (Android GLES) while supporting Vulkan and GLES.
- **Kotlin host (`crates/rune/android/src/main/java/dev/rune/`)**:
  - `RuneActivity`: full-screen edge-to-edge `Activity` requesting `Window.FEATURE_NO_TITLE` and `SOFT_INPUT_ADJUST_NOTHING`, reading the shared library name from manifest meta-data `dev.rune.lib_name`, moving task-root activities to the back on unhandled Back so state survives launcher re-entry, and launching Storage Access Framework document pickers for `RuneView`.
  - `RuneView`: `SurfaceView` + `Choreographer.FrameCallback` forwarding surface lifecycle, display density, system bars/display cutout, soft keyboard inset, dark mode, Back presses, `Intent.ACTION_VIEW` link launches, and Storage Access Framework document picks (`onDocumentPick`, `onDocumentPickActivityResult`, `deliverDocumentPick`, `openDocumentFd`).
  - `RuneClipboard` / `RuneClipboardProvider`: wraps Android's `ClipboardManager` for plain text, HTML, sensitive secrets (`ClipDescription.EXTRA_IS_SENSITIVE`), change revisions (`OnPrimaryClipChangedListener`), image paste via `ContentResolver`, and image copy via an unexported `ContentProvider`.
- **Document picker (`Tx::pick_file`, `Tx::pick_files`, `Tx::pick_save_file`)**:
  - Handle-based dialogs launch `ACTION_OPEN_DOCUMENT` (single or multi-select with filter extensions mapped to MIME types) or `ACTION_CREATE_DOCUMENT` (with the suggested file name).
  - Picked `content://` URIs are wrapped in `FileHandle::from_provider` backed by `ContentResolver.openFileDescriptor(..).detachFd()`, streaming directly through `std::fs::File` without copying into app cache.
  - Path-based dialogs (`open_file`, `open_files`, `save_file`, `open_folder`) are refused with a `FileDialogError`. Embedding hosts with their own `Activity` set `RuneView.onDocumentPick` and hand the result back via `onDocumentPickActivityResult` or `deliverDocumentPick`.
- **Insets and Back**:
  - Hardware system bars and display cutouts map to `Cx::safe_area()` (`[top, right, bottom, left]` in dp).
  - Soft keyboard overlap maps separately to `Cx::keyboard_inset()` (dp from bottom) so `El::on_edge_drag` bands stay bound to hardware chrome while views can pad by `cx.safe_area()[2].max(cx.keyboard_inset())` and focused text fields auto-scroll above the keyboard.
  - System Back always routes `Key::Escape` through a synchronous zero-dt frame and asks the runtime whether the key was consumed (`take_key_consumed`). Consumed (closing a modal/panel, blurring a focused text field, cancelling a drag, or an app `on_key` returning `true`) keeps the Activity open and schedules a redraw; unconsumed (including when a non-text control such as a button keeps focus) delegates to Android to leave the Activity.

## Prerequisites

- Rust targets:
  ```sh
  rustup target add aarch64-linux-android x86_64-linux-android
  ```
- Android SDK (`ANDROID_HOME`) with a platform (`platforms/android-*`) and build-tools (`aapt2`, `d8`, `zipalign`, `apksigner`).
- Android NDK (`ANDROID_NDK_LATEST_HOME` or `$ANDROID_HOME/ndk/*`).
- `kotlinc` and `keytool` (JDK) on `PATH`.

## Building and running `examples/android-hello`

Build a signed debug APK without Gradle:

```sh
bash scripts/android-run.sh --build-only --abi x86_64
```

Install and launch on a running emulator or connected device:

```sh
bash scripts/android-run.sh --abi x86_64       # x86_64 emulator
bash scripts/android-run.sh --abi arm64-v8a    # arm64 device or emulator
adb logcat -s Rune
```

## Writing an Android Rune app

Compile your crate as a `cdylib` and export the entry point with `rune::android_main!`:

```rust,ignore
use rune::{App, AppSpec, Cx, El, col, text};

fn view(_: &(), cx: &mut Cx<()>) -> El {
    let [top, right, bottom, left] = cx.safe_area();
    let bottom = bottom.max(cx.keyboard_inset());
    col().w_full().h_full().pt(top).pr(right).pb(bottom).pl(left).child(text("Hello"))
}

pub fn start() {
    App::new(AppSpec::new("Hello", (360.0, 800.0), || (), view)).run();
}

rune::android_main!(start);
```

Include `crates/rune/android/src/main/java` in your Android project and declare `dev.rune.RuneActivity` with `android:theme="@android:style/Theme.DeviceDefault.NoActionBar"` (avoiding a title bar on the starting window; `RuneActivity` also requests `Window.FEATURE_NO_TITLE`), `<meta-data android:name="dev.rune.lib_name" android:value="your_crate" />`, and `android:windowSoftInputMode="adjustNothing"`.

### Clipboard image copy (`RuneClipboardProvider`)

Plain text, HTML, sensitive secret copy, and image paste work out of the box. To also allow `Tx::copy_image` and rich image copies on Android, declare `dev.rune.RuneClipboardProvider` inside `<application>` in your `AndroidManifest.xml` (unexported, with temporary URI grants enabled so only the pasting app can read the image):

```xml
<provider
    android:name="dev.rune.RuneClipboardProvider"
    android:authorities="${applicationId}.rune.clipboard"
    android:exported="false"
    android:grantUriPermissions="true" />
```

When packaging directly with `aapt2` without Gradle, substitute your manifest `package` name for `${applicationId}` (for example `dev.rune.hello.rune.clipboard`). Apps that omit the `<provider>` entry receive `CopyImageError::Failed` on image copy without crashing or modifying the existing clipboard.
