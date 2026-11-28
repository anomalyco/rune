package dev.rune

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.view.Choreographer
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.WindowInsets

/**
 * Native Android [SurfaceView] hosting one Rune [LayerHost], driven on demand by [Choreographer].
 *
 * Loads [libName] via [System.loadLibrary] (which triggers `JNI_OnLoad` from `rune::android_main!`),
 * forwards surface lifecycle, display density, system-bar/cutout insets, separate IME keyboard
 * inset, dark mode, and back navigation to Rust, and tears down the GPU swapchain synchronously
 * inside [surfaceDestroyed].
 */
open class RuneView(
    context: Context,
    libName: String,
) : SurfaceView(context), SurfaceHolder.Callback, Choreographer.FrameCallback {

    private val mainHandler = Handler(Looper.getMainLooper())
    private var frameScheduled = false
    private val wakeRunnable = Runnable { nativeWake() }
    private val clipboard: RuneClipboard

    init {
        System.loadLibrary(libName)
        clipboard = RuneClipboard.getOrCreate(context)
        holder.addCallback(this)
        isFocusable = true
        isFocusableInTouchMode = true
        setOnApplyWindowInsetsListener { _, insets ->
            dispatchInsets(insets)
        }
        nativeInit()
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        requestApplyInsets()
    }

    private fun density(): Float {
        val d = resources.displayMetrics.density
        return if (d.isFinite() && d > 0f) d else 1f
    }

    private fun isDarkMode(): Boolean {
        val mask = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK
        return mask == Configuration.UI_MODE_NIGHT_YES
    }

    private fun dispatchInsets(insets: WindowInsets): WindowInsets {
        val scale = density()
        if (Build.VERSION.SDK_INT >= 30) {
            val bars = insets.getInsets(
                WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout(),
            )
            val ime = insets.getInsets(WindowInsets.Type.ime())
            nativeInsets(bars.top, bars.right, bars.bottom, bars.left, ime.bottom, scale)
            return WindowInsets.CONSUMED
        }
        @Suppress("DEPRECATION")
        nativeInsets(
            insets.systemWindowInsetTop,
            insets.systemWindowInsetRight,
            insets.systemWindowInsetBottom,
            insets.systemWindowInsetLeft,
            0,
            scale,
        )
        @Suppress("DEPRECATION")
        return insets.consumeSystemWindowInsets()
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        val frame = holder.surfaceFrame
        val w = if (frame.width() > 0) frame.width() else width
        val h = if (frame.height() > 0) frame.height() else height
        nativeSurfaceCreated(holder.surface, w, h, density(), isDarkMode())
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        nativeSurfaceChanged(width, height, density())
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        cancelFrame()
        nativeSurfaceDestroyed()
    }

    override fun doFrame(frameTimeNanos: Long) {
        frameScheduled = false
        nativeFrame()
    }

    /** Called by Rust (UI thread) when a redraw is needed on the next vsync. */
    fun scheduleFrame() {
        if (!frameScheduled) {
            frameScheduled = true
            Choreographer.getInstance().postFrameCallback(this)
        }
    }

    /** Called by Rust (UI thread) on pause, surface loss, or destroy. */
    fun cancelFrame() {
        if (frameScheduled) {
            frameScheduled = false
            Choreographer.getInstance().removeFrameCallback(this)
        }
        mainHandler.removeCallbacks(wakeRunnable)
    }

    /** Called by Rust (UI thread) when a runtime timer needs a wake after [delayMs] ms. */
    fun scheduleWakeAfter(delayMs: Long) {
        mainHandler.removeCallbacks(wakeRunnable)
        mainHandler.postDelayed(wakeRunnable, delayMs.coerceAtLeast(1L))
    }

    /** Called by Rust from any thread (via JNI) when a [Waker] or [AppHandle] wakes the app. */
    fun postWake() {
        mainHandler.post(wakeRunnable)
    }

    /**
     * Called by Rust (UI thread) after a drawn frame to play one
     * [android.view.HapticFeedbackConstants] effect on this view without override flags, requiring
     * no `VIBRATE` permission and following the system Touch Feedback setting.
     */
    fun performHaptic(feedbackConstant: Int) {
        performHapticFeedback(feedbackConstant)
    }

    /** Called by Rust during [nativeInit] to bind the process-wide clipboard wrapper. */
    fun getClipboard(): RuneClipboard = clipboard

    /**
     * Called by Rust (via JNI) to open an `http:`, `https:`, or `mailto:` link using
     * `Intent.ACTION_VIEW`. Returns `null` on success, or an error string when no app handles
     * the link or the launch fails.
     */
    fun openUrl(url: String): String? {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
        val target = findActivity(context)
        if (target == null) {
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        return try {
            (target ?: context).startActivity(intent)
            null
        } catch (_: ActivityNotFoundException) {
            "no app opens this link"
        } catch (e: RuntimeException) {
            e.message ?: e.javaClass.simpleName
        }
    }

    private fun findActivity(ctx: Context): Activity? {
        var cur: Context? = ctx
        while (cur is ContextWrapper) {
            if (cur is Activity) return cur
            cur = cur.baseContext
        }
        return null
    }

    fun onResume() {
        nativeResume()
    }

    fun onPause() {
        cancelFrame()
        nativePause()
    }

    fun onDarkModeChanged(dark: Boolean) {
        nativeDarkMode(dark)
    }

    /** Returns true if Rune consumed Back (by sending Escape to a focused element). */
    fun handleBack(): Boolean = nativeBack()

    fun onDestroy() {
        cancelFrame()
        nativeDestroy()
    }

    private external fun nativeInit()
    private external fun nativeSurfaceCreated(
        surface: Surface,
        width: Int,
        height: Int,
        density: Float,
        dark: Boolean,
    )
    private external fun nativeSurfaceChanged(width: Int, height: Int, density: Float)
    private external fun nativeSurfaceDestroyed()
    private external fun nativeFrame()
    private external fun nativeInsets(
        top: Int,
        right: Int,
        bottom: Int,
        left: Int,
        imeBottom: Int,
        density: Float,
    )
    private external fun nativeDarkMode(dark: Boolean)
    private external fun nativeResume()
    private external fun nativePause()
    private external fun nativeBack(): Boolean
    private external fun nativeWake()
    private external fun nativeDestroy()
}
