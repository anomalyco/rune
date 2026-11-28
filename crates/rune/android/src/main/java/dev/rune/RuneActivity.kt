package dev.rune

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.view.View
import android.view.Window
import android.view.WindowManager
import android.window.OnBackInvokedCallback
import android.window.OnBackInvokedDispatcher

/**
 * Default [Activity] hosting a full-screen edge-to-edge [RuneView].
 *
 * Reads the Rust shared library name from activity manifest meta-data `dev.rune.lib_name`
 * (defaulting to `rune_app`), requests [Window.FEATURE_NO_TITLE] so no system action bar covers
 * the edge-to-edge surface even when an app omits a `NoActionBar` theme, configures
 * `SOFT_INPUT_ADJUST_NOTHING` so soft keyboards report [dev.rune.RuneView] IME overlap without
 * resizing the layout viewport, forwards dark mode and lifecycle events, and delegates Back
 * presses to Rune (as `Key::Escape`). When Back is unconsumed, task-root activities move to the
 * back (preserving app state across launcher re-entry) while non-root activities finish.
 */
open class RuneActivity : Activity() {

    protected lateinit var runeView: RuneView
        private set

    private var lastDark: Boolean = false
    private var backCallback: Any? = null
    private var pendingPickRequestId: Long? = null

    /** Override to supply the Rust cdylib name without manifest meta-data. */
    protected open fun nativeLibraryName(): String {
        @Suppress("DEPRECATION")
        val info = packageManager.getActivityInfo(componentName, PackageManager.GET_META_DATA)
        return info.metaData?.getString(META_LIB_NAME) ?: DEFAULT_LIB_NAME
    }

    private fun isDark(config: Configuration): Boolean {
        return (config.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    }

    @Suppress("DEPRECATION")
    private fun configureEdgeToEdge() {
        window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING)
        if (Build.VERSION.SDK_INT >= 30) {
            window.setDecorFitsSystemWindows(false)
        } else {
            window.decorView.systemUiVisibility = (
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                )
        }
        window.statusBarColor = Color.TRANSPARENT
        window.navigationBarColor = Color.TRANSPARENT
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestWindowFeature(Window.FEATURE_NO_TITLE)
        configureEdgeToEdge()
        lastDark = isDark(resources.configuration)
        runeView = RuneView(this, nativeLibraryName())
        runeView.onDocumentPick = { requestId, intent ->
            launchDocumentPicker(requestId, intent)
        }
        setContentView(runeView)
        actionBar?.hide()

        if (Build.VERSION.SDK_INT >= 33) {
            val cb = OnBackInvokedCallback {
                if (!runeView.handleBack()) {
                    onUnhandledBack()
                }
            }
            backCallback = cb
            onBackInvokedDispatcher.registerOnBackInvokedCallback(
                OnBackInvokedDispatcher.PRIORITY_DEFAULT,
                cb,
            )
        }
    }

    override fun onResume() {
        super.onResume()
        runeView.onResume()
    }

    override fun onPause() {
        runeView.onPause()
        super.onPause()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        val dark = isDark(newConfig)
        if (dark != lastDark) {
            lastDark = dark
            runeView.onDarkModeChanged(dark)
        }
    }

    /** Starts a Storage Access Framework picker intent on behalf of [runeView]. */
    @Suppress("DEPRECATION")
    protected open fun launchDocumentPicker(requestId: Long, intent: Intent) {
        pendingPickRequestId = requestId
        try {
            startActivityForResult(intent, REQUEST_CODE_DOCUMENT_PICK)
        } catch (e: ActivityNotFoundException) {
            pendingPickRequestId = null
            runeView.deliverDocumentPickError(
                requestId,
                e.message ?: "no Android document picker activity is available",
            )
        } catch (e: SecurityException) {
            pendingPickRequestId = null
            runeView.deliverDocumentPickError(
                requestId,
                e.message ?: "permission denied launching Android document picker",
            )
        }
    }

    @Suppress("DEPRECATION")
    @Deprecated("Deprecated in Android 11; used to keep dev.rune free of AndroidX dependencies.")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == REQUEST_CODE_DOCUMENT_PICK) {
            val requestId = pendingPickRequestId
            pendingPickRequestId = null
            if (requestId != null) {
                runeView.onDocumentPickActivityResult(requestId, resultCode, data)
                return
            }
        }
        super.onActivityResult(requestCode, resultCode, data)
    }

    /**
     * Called when Rune did not consume a Back press.
     *
     * Matches Android 12+'s root-activity Back behaviour: moves a task-root activity to the back
     * so the activity, [runeView], and Rust app state survive returning from the launcher, and
     * finishes non-root activities (or falls back to [finish] if the task cannot be moved).
     */
    protected open fun onUnhandledBack() {
        if (isTaskRoot && moveTaskToBack(true)) {
            return
        }
        finish()
    }

    @Suppress("DEPRECATION")
    @Deprecated("Deprecated in Android 13; kept for API < 33 fallback.")
    override fun onBackPressed() {
        if (!runeView.handleBack()) {
            onUnhandledBack()
        }
    }

    override fun onDestroy() {
        if (Build.VERSION.SDK_INT >= 33) {
            (backCallback as? OnBackInvokedCallback)?.let { cb ->
                onBackInvokedDispatcher.unregisterOnBackInvokedCallback(cb)
            }
            backCallback = null
        }
        runeView.onDestroy()
        super.onDestroy()
    }

    companion object {
        const val META_LIB_NAME: String = "dev.rune.lib_name"
        const val DEFAULT_LIB_NAME: String = "rune_app"
        private const val REQUEST_CODE_DOCUMENT_PICK: Int = 0x5255
    }
}
