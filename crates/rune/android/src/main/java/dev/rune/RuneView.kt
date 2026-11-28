package dev.rune

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Rect
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import android.text.InputType
import android.view.Choreographer
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.ViewConfiguration
import android.view.WindowInsets
import android.view.accessibility.AccessibilityManager
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityNodeProvider
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import java.io.IOException

/**
 * Native Android [SurfaceView] hosting one Rune [LayerHost], driven on demand by [Choreographer].
 *
 * Loads [libName] via [System.loadLibrary] (which triggers `JNI_OnLoad` from `rune::android_main!`),
 * forwards surface lifecycle, touch pointers (including batched historical samples), the system
 * long-press timeout, display density, system-bar/cutout insets, separate IME keyboard inset,
 * dark mode, and back navigation to Rust, and tears down the GPU swapchain synchronously inside
 * [surfaceDestroyed].
 */
open class RuneView(
    context: Context,
    libName: String,
) : SurfaceView(context), SurfaceHolder.Callback, Choreographer.FrameCallback {

    private val mainHandler = Handler(Looper.getMainLooper())
    private var frameScheduled = false
    private var imeEnabled = false
    private var imeInputType: Int = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
    private var imeOptions: Int = EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_FULLSCREEN
    private val wakeRunnable = Runnable { nativeWake() }
    private val timerWakeRunnable = Runnable { nativeWake() }
    private var a11yWakeRunnable: Runnable? = null
    private val clipboard: RuneClipboard
    private val selectionOverlay: RuneSelectionOverlay
    private val accessibilityManager: AccessibilityManager? =
        context.getSystemService(Context.ACCESSIBILITY_SERVICE) as? AccessibilityManager
    private val tmpRect = Rect()
    private val tmpLocation = IntArray(2)

    private val a11yProvider = object : AccessibilityNodeProvider() {
        override fun createAccessibilityNodeInfo(virtualViewId: Int): AccessibilityNodeInfo? {
            val info = nativeA11yCreateNodeInfo(virtualViewId) ?: return null
            if (virtualViewId == HOST_VIEW_ID) {
                makeRootPassive(info)
            } else {
                applyNodeFlags(info)
            }
            return info
        }

        override fun findFocus(focus: Int): AccessibilityNodeInfo? {
            val info = nativeA11yFindFocus(focus) ?: return null
            applyNodeFlags(info)
            return info
        }

        override fun performAction(
            virtualViewId: Int,
            action: Int,
            arguments: Bundle?,
        ): Boolean {
            if (virtualViewId == HOST_VIEW_ID &&
                (action == AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS ||
                    action == AccessibilityNodeInfo.ACTION_CLEAR_ACCESSIBILITY_FOCUS)
            ) {
                return false
            }
            return nativeA11yPerformAction(virtualViewId, action, arguments)
        }
    }

    /**
     * Optional launcher for Android Storage Access Framework document pickers
     * (`ACTION_OPEN_DOCUMENT` / `ACTION_CREATE_DOCUMENT`).
     *
     * [RuneActivity] wires this automatically. An embedding host with its own [Activity] sets
     * this callback, starts `intent` for a result, and forwards the outcome back via
     * [onDocumentPickActivityResult], [deliverDocumentPick], [deliverDocumentPickCancelled],
     * or [deliverDocumentPickError].
     */
    var onDocumentPick: ((requestId: Long, intent: Intent) -> Unit)? = null

    init {
        System.loadLibrary(libName)
        clipboard = RuneClipboard.getOrCreate(context)
        selectionOverlay = RuneSelectionOverlay(
            hostView = this,
            onHandleGrab = { kind, xPx, yPx -> nativeSelectionHandleGrab(kind, xPx, yPx) },
            onHandleMove = { xPx, yPx -> nativeSelectionHandleMove(xPx, yPx) },
            onHandleRelease = { nativeSelectionHandleRelease() },
            onCaretTap = { nativeSelectionCaretTap() },
            onToolbarAction = { action -> nativeSelectionToolbarAction(action) },
        )
        holder.addCallback(this)
        isFocusable = true
        isFocusableInTouchMode = true
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_YES
        setOnApplyWindowInsetsListener { _, insets ->
            dispatchInsets(insets)
        }
        nativeInit()
        syncLongPressTimeout()
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        requestApplyInsets()
    }

    override fun onDetachedFromWindow() {
        selectionOverlay.dismissAll()
        super.onDetachedFromWindow()
    }

    override fun getAccessibilityNodeProvider(): AccessibilityNodeProvider = a11yProvider

    override fun onInitializeAccessibilityNodeInfo(info: AccessibilityNodeInfo) {
        super.onInitializeAccessibilityNodeInfo(info)
        makeRootPassive(info)
    }

    override fun dispatchHoverEvent(event: MotionEvent): Boolean {
        if (accessibilityManager?.isTouchExplorationEnabled == true &&
            nativeA11yHover(event.actionMasked, event.x, event.y)
        ) {
            return true
        }
        return super.dispatchHoverEvent(event)
    }

    private fun makeRootPassive(info: AccessibilityNodeInfo) {
        info.isFocusable = false
        info.isFocused = false
        info.isClickable = false
        info.isLongClickable = false
        info.removeAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_FOCUS)
        info.removeAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_CLICK)
        info.removeAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_ACCESSIBILITY_FOCUS)
        info.removeAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_CLEAR_ACCESSIBILITY_FOCUS)
        if (Build.VERSION.SDK_INT >= 28) {
            info.isScreenReaderFocusable = false
            val title = info.contentDescription
            if (!title.isNullOrEmpty() && info.paneTitle.isNullOrEmpty()) {
                info.paneTitle = title
            }
        }
        info.contentDescription = null
    }

    private fun applyNodeFlags(info: AccessibilityNodeInfo) {
        if (Build.VERSION.SDK_INT < 28) return
        val cls = info.className?.toString() ?: return
        if (cls != "android.widget.TextView" &&
            cls != "android.view.View" &&
            cls != "android.widget.ImageView"
        ) {
            return
        }
        tmpRect.setEmpty()
        info.getBoundsInScreen(tmpRect)
        val hasBounds =
            tmpRect.left != 0 || tmpRect.top != 0 || tmpRect.right != 0 || tmpRect.bottom != 0
        getLocationOnScreen(tmpLocation)
        val clickable =
            info.actionList.contains(AccessibilityNodeInfo.AccessibilityAction.ACTION_CLICK)
        val flags = nativeA11yNodeFlags(
            cls,
            tmpRect.left - tmpLocation[0],
            tmpRect.top - tmpLocation[1],
            tmpRect.right - tmpLocation[0],
            tmpRect.bottom - tmpLocation[1],
            hasBounds,
            info.contentDescription?.toString(),
            info.text?.toString(),
            info.childCount,
            info.isFocusable,
            clickable,
        )
        if ((flags and FLAG_SCREEN_READER_FOCUSABLE) != 0) {
            info.isScreenReaderFocusable = true
        }
        if ((flags and FLAG_HEADING) != 0) {
            info.isHeading = true
        }
    }

    override fun onCheckIsTextEditor(): Boolean = imeEnabled

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection? {
        if (!imeEnabled) {
            return null
        }
        outAttrs.inputType = imeInputType
        outAttrs.imeOptions = imeOptions
        val sel = nativeGetSelection()
        if (sel != null && sel.size >= 2) {
            outAttrs.initialSelStart = sel[0]
            outAttrs.initialSelEnd = sel[1]
        }
        if (Build.VERSION.SDK_INT >= 30) {
            val text = nativeGetWindowText()
            if (text != null) {
                outAttrs.setInitialSurroundingText(text)
            }
        }
        return RuneInputConnection(this)
    }

    /**
     * Called by Rust (UI thread) after frames or lifecycle changes when text input enables,
     * disables, switches editor kind, resets composition (including paragraph changes), or
     * re-shows after a tap on a focused field.
     */
    fun setImeState(enabled: Boolean, restart: Boolean, inputType: Int, imeOptions: Int) {
        val wasEnabled = imeEnabled
        imeEnabled = enabled
        if (enabled) {
            imeInputType = inputType
            this.imeOptions = imeOptions
        }
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
            ?: return
        if (enabled) {
            if (!hasFocus()) {
                requestFocus()
            }
            if (!wasEnabled || restart) {
                imm.restartInput(this)
            }
            if (!imm.showSoftInput(this, 0)) {
                post {
                    if (imeEnabled && windowToken != null) {
                        if (!hasFocus()) {
                            requestFocus()
                        }
                        imm.restartInput(this)
                        imm.showSoftInput(this, 0)
                    }
                }
            }
        } else if (wasEnabled) {
            windowToken?.let { token ->
                imm.hideSoftInputFromWindow(token, 0)
            }
            if (restart) {
                imm.restartInput(this)
            }
        }
    }

    /**
     * Called by Rust (UI thread) after frames when the active paragraph window's UTF-16 selection
     * or composing span changes without requiring `restartInput`.
     */
    fun updateImeSelection(selStart: Int, selEnd: Int, composingStart: Int, composingEnd: Int) {
        if (!imeEnabled) return
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
            ?: return
        imm.updateSelection(this, selStart, selEnd, composingStart, composingEnd)
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (dispatchKeyDown(event)) {
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean {
        if (dispatchKeyUp(event)) {
            return true
        }
        return super.onKeyUp(keyCode, event)
    }

    @Suppress("DEPRECATION")
    override fun onKeyMultiple(keyCode: Int, repeatCount: Int, event: KeyEvent): Boolean {
        if (keyCode == KeyEvent.KEYCODE_UNKNOWN) {
            val chars = event.characters
            if (!chars.isNullOrEmpty()) {
                nativeCommitText(chars)
                return true
            }
        }
        return super.onKeyMultiple(keyCode, repeatCount, event)
    }

    internal fun onImeCommitText(text: String) {
        nativeCommitText(text)
    }

    internal fun onImeSetComposingText(text: String, newCursorPosition: Int) {
        nativeSetComposingText(text, newCursorPosition)
    }

    internal fun onImeSetComposingRegion(start: Int, end: Int) {
        nativeSetComposingRegion(start, end)
    }

    internal fun onImeFinishComposingText() {
        nativeFinishComposingText()
    }

    internal fun onImeSetSelection(start: Int, end: Int) {
        nativeSetSelection(start, end)
    }

    internal fun onImeDeleteSurroundingText(beforeLength: Int, afterLength: Int) {
        nativeDeleteSurroundingText(beforeLength, afterLength)
    }

    internal fun onImeDeleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int) {
        nativeDeleteSurroundingTextInCodePoints(beforeLength, afterLength)
    }

    internal fun onImeGetTextBeforeCursor(n: Int): String? {
        return nativeGetTextBeforeCursor(n)
    }

    internal fun onImeGetTextAfterCursor(n: Int): String? {
        return nativeGetTextAfterCursor(n)
    }

    internal fun onImeGetSelectedText(): String? {
        return nativeGetSelectedText()
    }

    internal fun onImeGetWindowText(): String? {
        return nativeGetWindowText()
    }

    internal fun onImeGetSelection(): IntArray? {
        return nativeGetSelection()
    }

    internal fun onImeEditorAction(editorAction: Int): Boolean {
        return nativeEditorAction(editorAction)
    }

    @Suppress("DEPRECATION")
    internal fun onImeKeyEvent(event: KeyEvent): Boolean {
        return when (event.action) {
            KeyEvent.ACTION_DOWN -> dispatchKeyDown(event)
            KeyEvent.ACTION_UP -> dispatchKeyUp(event)
            KeyEvent.ACTION_MULTIPLE -> {
                if (event.keyCode == KeyEvent.KEYCODE_UNKNOWN) {
                    val chars = event.characters
                    if (!chars.isNullOrEmpty()) {
                        nativeCommitText(chars)
                        return true
                    }
                }
                false
            }
            else -> false
        }
    }

    private fun dispatchKeyDown(event: KeyEvent): Boolean {
        return nativeKeyDown(
            event.keyCode,
            event.metaState,
            event.unicodeChar,
            event.getUnicodeChar(0),
            event.repeatCount > 0,
        )
    }

    private fun dispatchKeyUp(event: KeyEvent): Boolean {
        return nativeKeyUp(
            event.keyCode,
            event.metaState,
            event.unicodeChar,
            event.getUnicodeChar(0),
        )
    }

    private fun density(): Float {
        val d = resources.displayMetrics.density
        return if (d.isFinite() && d > 0f) d else 1f
    }

    private fun isDarkMode(): Boolean {
        val mask = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK
        return mask == Configuration.UI_MODE_NIGHT_YES
    }

    private fun syncLongPressTimeout() {
        nativeLongPressTimeout(ViewConfiguration.getLongPressTimeout())
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

    override fun performClick(): Boolean {
        super.performClick()
        return true
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        val action = event.actionMasked
        val flags = event.flags
        val count = event.pointerCount
        when (action) {
            MotionEvent.ACTION_DOWN -> {
                if (count <= 0) return false
                nativeTouch(
                    action,
                    flags,
                    event.getPointerId(0),
                    event.getX(0),
                    event.getY(0),
                    event.eventTime,
                )
                return true
            }
            MotionEvent.ACTION_POINTER_DOWN -> {
                val idx = event.actionIndex
                if (idx !in 0 until count) return false
                nativeTouch(
                    action,
                    flags,
                    event.getPointerId(idx),
                    event.getX(idx),
                    event.getY(idx),
                    event.eventTime,
                )
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                val history = event.historySize
                for (h in 0 until history) {
                    val t = event.getHistoricalEventTime(h)
                    for (p in 0 until count) {
                        nativeTouch(
                            MotionEvent.ACTION_MOVE,
                            flags,
                            event.getPointerId(p),
                            event.getHistoricalX(p, h),
                            event.getHistoricalY(p, h),
                            t,
                        )
                    }
                }
                val t = event.eventTime
                for (p in 0 until count) {
                    nativeTouch(
                        MotionEvent.ACTION_MOVE,
                        flags,
                        event.getPointerId(p),
                        event.getX(p),
                        event.getY(p),
                        t,
                    )
                }
                return true
            }
            MotionEvent.ACTION_POINTER_UP -> {
                val idx = event.actionIndex
                if (idx !in 0 until count) return false
                nativeTouch(
                    action,
                    flags,
                    event.getPointerId(idx),
                    event.getX(idx),
                    event.getY(idx),
                    event.eventTime,
                )
                return true
            }
            MotionEvent.ACTION_UP -> {
                if (count <= 0) return false
                nativeTouch(
                    action,
                    flags,
                    event.getPointerId(0),
                    event.getX(0),
                    event.getY(0),
                    event.eventTime,
                )
                if ((flags and FLAG_CANCELED) == 0) {
                    performClick()
                }
                return true
            }
            MotionEvent.ACTION_CANCEL -> {
                val t = event.eventTime
                for (p in 0 until count) {
                    nativeTouch(
                        MotionEvent.ACTION_CANCEL,
                        flags,
                        event.getPointerId(p),
                        event.getX(p),
                        event.getY(p),
                        t,
                    )
                }
                return true
            }
            else -> return super.onTouchEvent(event)
        }
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
        selectionOverlay.dismissAll()
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
        mainHandler.removeCallbacks(timerWakeRunnable)
        cancelA11yWake()
    }

    /** Called by Rust (UI thread) when a runtime timer needs a wake after [delayMs] ms. */
    fun scheduleWakeAfter(delayMs: Long) {
        mainHandler.removeCallbacks(timerWakeRunnable)
        mainHandler.postDelayed(timerWakeRunnable, delayMs.coerceAtLeast(1L))
    }

    /** Called by Rust (UI thread) when scroll-paced accessibility sync needs a delayed wake. */
    fun scheduleA11yWakeAfter(attempt: Long, delayMs: Long) {
        cancelA11yWake()
        val runnable = Runnable {
            a11yWakeRunnable = null
            nativeA11yWake(attempt)
        }
        a11yWakeRunnable = runnable
        mainHandler.postDelayed(runnable, delayMs.coerceAtLeast(1L))
    }

    /** Called by Rust (UI thread) to cancel any pending scroll-paced accessibility wake. */
    fun cancelA11yWake() {
        a11yWakeRunnable?.let { mainHandler.removeCallbacks(it) }
        a11yWakeRunnable = null
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

    /**
     * Called by Rust (UI thread) when the app invokes `Tx::pick_file`, `Tx::pick_files`, or
     * `Tx::pick_save_file`.
     */
    fun startDocumentPick(
        requestId: Long,
        isSave: Boolean,
        multiple: Boolean,
        fileName: String,
        mimeType: String,
        extraMimeTypes: Array<String>,
    ) {
        val intent = if (isSave) {
            Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = mimeType.ifEmpty { "application/octet-stream" }
                if (fileName.isNotEmpty()) {
                    putExtra(Intent.EXTRA_TITLE, fileName)
                }
            }
        } else {
            Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = mimeType.ifEmpty { "*/*" }
                if (extraMimeTypes.isNotEmpty()) {
                    putExtra(Intent.EXTRA_MIME_TYPES, extraMimeTypes)
                }
                if (multiple) {
                    putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
                }
            }
        }
        val launcher = onDocumentPick
        if (launcher == null) {
            nativeFilePickError(requestId, "no Android document picker launcher is registered on RuneView")
            return
        }
        try {
            launcher(requestId, intent)
        } catch (t: Throwable) {
            nativeFilePickError(requestId, t.message ?: t.javaClass.simpleName)
        }
    }

    /**
     * Opens a SAF `content://` URI with [mode] (`"r"` or `"wt"`) and returns a detached POSIX
     * file descriptor owned by the caller. Called by Rust from worker threads via JNI.
     */
    fun openDocumentFd(uriString: String, mode: String): Int {
        val uri = Uri.parse(uriString)
        val pfd = context.applicationContext.contentResolver.openFileDescriptor(uri, mode)
            ?: throw IOException("ContentResolver returned null descriptor for $uriString")
        return pfd.detachFd()
    }

    /** Forwards a raw [Activity.onActivityResult] from a document picker back to Rust. */
    fun onDocumentPickActivityResult(requestId: Long, resultCode: Int, data: Intent?) {
        if (resultCode != Activity.RESULT_OK || data == null) {
            nativeFilePickCancelled(requestId)
            return
        }
        val uris = ArrayList<Uri>()
        val clip = data.clipData
        if (clip != null && clip.itemCount > 0) {
            for (i in 0 until clip.itemCount) {
                clip.getItemAt(i)?.uri?.let { uris.add(it) }
            }
        } else {
            data.data?.let { uris.add(it) }
        }
        if (uris.isEmpty()) {
            nativeFilePickCancelled(requestId)
            return
        }
        deliverDocumentPick(requestId, uris)
    }

    /** Delivers one or more picked document [Uri]s back to Rust. */
    fun deliverDocumentPick(requestId: Long, uris: List<Uri>) {
        if (uris.isEmpty()) {
            nativeFilePickCancelled(requestId)
            return
        }
        val uriStrings = Array(uris.size) { i -> uris[i].toString() }
        val names = Array(uris.size) { i -> queryDisplayName(uris[i]) }
        nativeFilePickResult(requestId, uriStrings, names)
    }

    /** Delivers a user cancellation for [requestId] back to Rust. */
    fun deliverDocumentPickCancelled(requestId: Long) {
        nativeFilePickCancelled(requestId)
    }

    /** Delivers a launcher or picker error for [requestId] back to Rust. */
    fun deliverDocumentPickError(requestId: Long, message: String) {
        nativeFilePickError(requestId, message)
    }

    private fun queryDisplayName(uri: Uri): String {
        try {
            context.applicationContext.contentResolver.query(
                uri,
                arrayOf(OpenableColumns.DISPLAY_NAME),
                null,
                null,
                null,
            )?.use { cursor ->
                val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (idx >= 0 && cursor.moveToFirst()) {
                    val name = cursor.getString(idx)
                    if (!name.isNullOrEmpty()) {
                        return name
                    }
                }
            }
        } catch (_: Throwable) {
            // Fall back to URI segment when a provider refuses metadata queries.
        }
        return uri.lastPathSegment
            ?.substringAfterLast('/')
            ?.substringAfterLast(':')
            ?.ifEmpty { null }
            ?: "document"
    }

    fun onResume() {
        syncLongPressTimeout()
        nativeResume()
    }

    fun onPause() {
        selectionOverlay.dismissAll()
        cancelFrame()
        nativePause()
    }

    fun onDarkModeChanged(dark: Boolean) {
        nativeDarkMode(dark)
    }

    /** Returns true if Rune consumed Back (routed as Escape); false lets the Activity go back. */
    fun handleBack(): Boolean = nativeBack()

    /**
     * Called by Rust (UI thread) to position or hide native selection handles, the drag magnifier,
     * and the floating `ActionMode` toolbar in view-local pixels.
     */
    @Suppress("LongParameterList")
    fun updateSelectionUi(
        hasStart: Boolean, startX: Float, startTop: Float, startBottom: Float,
        hasEnd: Boolean, endX: Float, endTop: Float, endBottom: Float,
        hasCaret: Boolean, caretX: Float, caretTop: Float, caretBottom: Float,
        hasMagnifier: Boolean, magX: Float, magY: Float,
        toolbarActions: Int,
        rectLeft: Float, rectTop: Float, rectRight: Float, rectBottom: Float,
    ) {
        selectionOverlay.update(
            hasStart, startX, startTop, startBottom,
            hasEnd, endX, endTop, endBottom,
            hasCaret, caretX, caretTop, caretBottom,
            hasMagnifier, magX, magY,
            toolbarActions,
            rectLeft, rectTop, rectRight, rectBottom,
        )
    }

    fun onDestroy() {
        selectionOverlay.dismissAll()
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
    private external fun nativeTouch(
        action: Int,
        flags: Int,
        pointerId: Int,
        xPx: Float,
        yPx: Float,
        timeMs: Long,
    )
    private external fun nativeLongPressTimeout(timeoutMs: Int)
    private external fun nativeWake()
    private external fun nativeDestroy()
    private external fun nativeFilePickResult(
        requestId: Long,
        uris: Array<String>,
        names: Array<String>,
    )
    private external fun nativeFilePickCancelled(requestId: Long)
    private external fun nativeFilePickError(requestId: Long, message: String)
    private external fun nativeCommitText(text: String)
    private external fun nativeSetComposingText(text: String, newCursorPosition: Int)
    private external fun nativeSetComposingRegion(start: Int, end: Int)
    private external fun nativeFinishComposingText()
    private external fun nativeSetSelection(start: Int, end: Int)
    private external fun nativeDeleteSurroundingText(beforeLength: Int, afterLength: Int)
    private external fun nativeDeleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int)
    private external fun nativeGetTextBeforeCursor(n: Int): String?
    private external fun nativeGetTextAfterCursor(n: Int): String?
    private external fun nativeGetSelectedText(): String?
    private external fun nativeGetWindowText(): String?
    private external fun nativeGetSelection(): IntArray?
    private external fun nativeEditorAction(actionCode: Int): Boolean
    private external fun nativeKeyDown(
        keyCode: Int,
        metaState: Int,
        unicodeChar: Int,
        unmodifiedChar: Int,
        repeat: Boolean,
    ): Boolean
    private external fun nativeKeyUp(
        keyCode: Int,
        metaState: Int,
        unicodeChar: Int,
        unmodifiedChar: Int,
    ): Boolean
    private external fun nativeA11yCreateNodeInfo(virtualViewId: Int): AccessibilityNodeInfo?
    private external fun nativeA11yFindFocus(focusType: Int): AccessibilityNodeInfo?
    private external fun nativeA11yPerformAction(
        virtualViewId: Int,
        action: Int,
        arguments: Bundle?,
    ): Boolean
    private external fun nativeA11yHover(action: Int, x: Float, y: Float): Boolean
    private external fun nativeA11yWake(attempt: Long)
    private external fun nativeA11yNodeFlags(
        className: String,
        left: Int,
        top: Int,
        right: Int,
        bottom: Int,
        hasBounds: Boolean,
        contentDescription: String?,
        text: String?,
        childCount: Int,
        focusable: Boolean,
        clickable: Boolean,
    ): Int
    private external fun nativeSelectionHandleGrab(kind: Int, xPx: Float, yPx: Float)
    private external fun nativeSelectionHandleMove(xPx: Float, yPx: Float)
    private external fun nativeSelectionHandleRelease()
    private external fun nativeSelectionCaretTap()
    private external fun nativeSelectionToolbarAction(action: Int)

    private companion object {
        /** API 33+ MotionEvent.FLAG_CANCELED, safe to mask on API 26+. */
        private const val FLAG_CANCELED: Int = 0x20
        private const val FLAG_SCREEN_READER_FOCUSABLE = 1 shl 0
        private const val FLAG_HEADING = 1 shl 1
    }
}
