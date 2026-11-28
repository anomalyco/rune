package dev.rune

import android.content.Context
import android.graphics.Canvas
import android.graphics.Rect
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.view.ActionMode
import android.view.Gravity
import android.view.Menu
import android.view.MenuItem
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.widget.Magnifier
import android.widget.PopupWindow
import kotlin.math.hypot
import kotlin.math.roundToInt

/**
 * Native Android text-selection overlay anchored to a [RuneView]:
 * - theme `textSelectHandleLeft`, `textSelectHandleRight`, and `textSelectHandle` drawables in
 *   48dp x 48dp [PopupWindow] touch targets,
 * - floating [ActionMode] (`ActionMode.TYPE_FLOATING`) using Android's localized Cut/Copy/Paste/
 *   Select All strings and a content rect cleared past the foot of the 48dp handle targets,
 * - [Magnifier] (API 28+) following the active selection line while a handle is dragged.
 */
internal class RuneSelectionOverlay(
    private val hostView: View,
    private val onHandleGrab: (kind: Int, xPx: Float, yPx: Float) -> Unit,
    private val onHandleMove: (xPx: Float, yPx: Float) -> Unit,
    private val onHandleRelease: () -> Unit,
    private val onCaretTap: () -> Unit,
    private val onToolbarAction: (actionCode: Int) -> Unit,
) {
    private val startHandle = HandlePopup(HANDLE_START, android.R.attr.textSelectHandleLeft, 0.75f)
    private val endHandle = HandlePopup(HANDLE_END, android.R.attr.textSelectHandleRight, 0.25f)
    private val caretHandle = HandlePopup(HANDLE_CARET, android.R.attr.textSelectHandle, 0.5f)

    private var actionMode: ActionMode? = null
    private var suppressDestroyCallback: Boolean = false
    private var toolbarActions: Int = 0
    private val toolbarRect = Rect()
    private var magnifier: Any? = null

    private val actionModeCallback = object : ActionMode.Callback2() {
        override fun onCreateActionMode(mode: ActionMode, menu: Menu): Boolean {
            populateMenu(menu, toolbarActions)
            return menu.size() > 0
        }

        override fun onPrepareActionMode(mode: ActionMode, menu: Menu): Boolean {
            populateMenu(menu, toolbarActions)
            return true
        }

        override fun onActionItemClicked(mode: ActionMode, item: MenuItem): Boolean {
            val code = when (item.itemId) {
                android.R.id.cut -> TOOLBAR_CUT
                android.R.id.copy -> TOOLBAR_COPY
                android.R.id.paste -> TOOLBAR_PASTE
                android.R.id.selectAll -> TOOLBAR_SELECT_ALL
                else -> return false
            }
            onToolbarAction(code)
            return true
        }

        override fun onGetContentRect(mode: ActionMode, view: View, outRect: Rect) {
            outRect.set(toolbarRect)
        }

        override fun onDestroyActionMode(mode: ActionMode) {
            if (actionMode === mode) {
                actionMode = null
            }
            if (!suppressDestroyCallback) {
                toolbarActions = 0
                onToolbarAction(TOOLBAR_DISMISSED)
            }
        }
    }

    @Suppress("LongParameterList")
    fun update(
        hasStart: Boolean, startX: Float, startTop: Float, startBottom: Float,
        hasEnd: Boolean, endX: Float, endTop: Float, endBottom: Float,
        hasCaret: Boolean, caretX: Float, caretTop: Float, caretBottom: Float,
        hasMagnifier: Boolean, magX: Float, magY: Float,
        actions: Int,
        rectLeft: Float, rectTop: Float, rectRight: Float, rectBottom: Float,
    ) {
        if (!hostView.isAttachedToWindow || hostView.windowToken == null) {
            dismissAll()
            return
        }
        startHandle.update(hasStart, startX, startTop, startBottom)
        endHandle.update(hasEnd, endX, endTop, endBottom)
        caretHandle.update(hasCaret, caretX, caretTop, caretBottom)
        updateMagnifier(hasMagnifier, magX, magY)
        updateToolbar(actions, rectLeft, rectTop, rectRight, rectBottom)
    }

    fun dismissAll() {
        startHandle.dismiss()
        endHandle.dismiss()
        caretHandle.dismiss()
        dismissMagnifier()
        finishActionModeSilently()
    }

    private fun populateMenu(menu: Menu, actions: Int) {
        menu.clear()
        if ((actions and ACTION_CUT) != 0) {
            menu.add(Menu.NONE, android.R.id.cut, 0, android.R.string.cut)
                .setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM)
        }
        if ((actions and ACTION_COPY) != 0) {
            menu.add(Menu.NONE, android.R.id.copy, 1, android.R.string.copy)
                .setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM)
        }
        if ((actions and ACTION_PASTE) != 0) {
            menu.add(Menu.NONE, android.R.id.paste, 2, android.R.string.paste)
                .setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM)
        }
        if ((actions and ACTION_SELECT_ALL) != 0) {
            menu.add(Menu.NONE, android.R.id.selectAll, 3, android.R.string.selectAll)
                .setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM)
        }
    }

    private fun updateToolbar(
        actions: Int,
        left: Float,
        top: Float,
        right: Float,
        bottom: Float,
    ) {
        if (actions == 0) {
            finishActionModeSilently()
            return
        }
        val newLeft = left.roundToInt()
        val newTop = top.roundToInt()
        val newRight = right.roundToInt().coerceAtLeast(newLeft + 1)
        val newBottom = bottom.roundToInt().coerceAtLeast(newTop + 1)
        val rectChanged = toolbarRect.left != newLeft ||
            toolbarRect.top != newTop ||
            toolbarRect.right != newRight ||
            toolbarRect.bottom != newBottom
        val actionsChanged = toolbarActions != actions
        toolbarRect.set(newLeft, newTop, newRight, newBottom)
        toolbarActions = actions

        val existing = actionMode
        if (existing == null) {
            actionMode = try {
                hostView.startActionMode(actionModeCallback, ActionMode.TYPE_FLOATING)
            } catch (_: Throwable) {
                null
            }
        } else {
            if (actionsChanged) {
                existing.invalidate()
            }
            if (rectChanged) {
                existing.invalidateContentRect()
            }
        }
    }

    private fun finishActionModeSilently() {
        toolbarActions = 0
        val mode = actionMode ?: return
        actionMode = null
        suppressDestroyCallback = true
        try {
            mode.finish()
        } catch (_: Throwable) {
            // Ignore framework teardown races.
        } finally {
            suppressDestroyCallback = false
        }
    }

    private fun updateMagnifier(show: Boolean, xPx: Float, yPx: Float) {
        if (Build.VERSION.SDK_INT < 28) return
        if (!show) {
            dismissMagnifier()
            return
        }
        val mag = ensureMagnifier() ?: return
        try {
            (mag as Magnifier).show(xPx, yPx)
        } catch (_: Throwable) {
            // Ignore magnifier failures when surface is transitioning.
        }
    }

    private fun ensureMagnifier(): Any? {
        if (Build.VERSION.SDK_INT < 28) return null
        magnifier?.let { return it }
        val created: Any? = try {
            if (Build.VERSION.SDK_INT >= 29) {
                Magnifier.Builder(hostView).build()
            } else {
                @Suppress("DEPRECATION")
                Magnifier(hostView)
            }
        } catch (_: Throwable) {
            null
        }
        magnifier = created
        return created
    }

    private fun dismissMagnifier() {
        if (Build.VERSION.SDK_INT < 28) return
        val mag = magnifier ?: return
        try {
            (mag as Magnifier).dismiss()
        } catch (_: Throwable) {
            // Ignore teardown races.
        }
    }

    private fun resolveHandleDrawable(attr: Int): Drawable {
        val ctx: Context = hostView.context
        val ta = ctx.obtainStyledAttributes(intArrayOf(attr))
        val drawable = try {
            ta.getDrawable(0)?.mutate()
        } finally {
            ta.recycle()
        }
        if (drawable != null) return drawable
        val accent = resolveAccentColor(ctx)
        val density = ctx.resources.displayMetrics.density.coerceAtLeast(1f)
        val sizePx = (22f * density).roundToInt().coerceAtLeast(1)
        return GradientDrawable().apply {
            shape = GradientDrawable.OVAL
            setColor(accent)
            setSize(sizePx, sizePx)
        }
    }

    private fun resolveAccentColor(ctx: Context): Int {
        val ta = ctx.obtainStyledAttributes(intArrayOf(android.R.attr.colorAccent))
        return try {
            ta.getColor(0, 0xFF1A73E8.toInt())
        } finally {
            ta.recycle()
        }
    }

    private inner class HandlePopup(
        private val kind: Int,
        private val drawableAttr: Int,
        private val hotspotRatio: Float,
    ) {
        private var popup: PopupWindow? = null
        private var handleView: HandleView? = null
        private val windowLoc = IntArray(2)
        private val screenLoc = IntArray(2)
        private var anchorX: Float = 0f
        private var anchorTop: Float = 0f
        private var anchorBottom: Float = 0f
        private var dragging: Boolean = false
        private var downRawX: Float = 0f
        private var downRawY: Float = 0f
        private var grabOffsetX: Float = 0f
        private var grabOffsetY: Float = 0f

        private fun targetSizePx(): Int {
            val d = hostView.resources.displayMetrics.density.coerceAtLeast(1f)
            return (HANDLE_TARGET_DP * d).roundToInt().coerceAtLeast(1)
        }

        private fun ensurePopup(): PopupWindow {
            popup?.let { return it }
            val sizePx = targetSizePx()
            val view = HandleView(hostView.context, resolveHandleDrawable(drawableAttr), hotspotRatio)
            view.setOnTouchListener { _, event -> handleTouch(event) }
            handleView = view
            val win = PopupWindow(view, sizePx, sizePx, false).apply {
                isTouchable = true
                isFocusable = false
                isClippingEnabled = false
                setBackgroundDrawable(null)
            }
            popup = win
            return win
        }

        fun update(visible: Boolean, xPx: Float, topPx: Float, bottomPx: Float) {
            if (!visible) {
                dismiss()
                return
            }
            anchorX = xPx
            anchorTop = topPx
            anchorBottom = bottomPx
            val win = ensurePopup()
            val sizePx = targetSizePx()
            val hotspotX = sizePx * hotspotRatio
            hostView.getLocationInWindow(windowLoc)
            val winX = windowLoc[0] + (xPx - hotspotX).roundToInt()
            val winY = windowLoc[1] + bottomPx.roundToInt()
            try {
                if (win.isShowing) {
                    win.update(winX, winY, sizePx, sizePx)
                } else {
                    win.width = sizePx
                    win.height = sizePx
                    win.showAtLocation(hostView, Gravity.NO_GRAVITY, winX, winY)
                }
            } catch (_: Throwable) {
                // Ignore WindowManager token races during activity transitions.
            }
        }

        fun dismiss() {
            dragging = false
            val win = popup ?: return
            if (win.isShowing) {
                try {
                    win.dismiss()
                } catch (_: Throwable) {
                    // Ignore WindowManager teardown races.
                }
            }
        }

        private fun handleTouch(event: MotionEvent): Boolean {
            hostView.getLocationOnScreen(screenLoc)
            val localX = event.rawX - screenLoc[0]
            val localY = event.rawY - screenLoc[1]
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downRawX = event.rawX
                    downRawY = event.rawY
                    val midY = (anchorTop + anchorBottom) * 0.5f
                    grabOffsetX = localX - anchorX
                    grabOffsetY = localY - midY
                    if (kind == HANDLE_CARET) {
                        dragging = false
                    } else {
                        dragging = true
                        onHandleGrab(kind, anchorX, midY)
                    }
                    return true
                }
                MotionEvent.ACTION_MOVE -> {
                    if (!dragging && kind == HANDLE_CARET) {
                        val slop = ViewConfiguration.get(hostView.context).scaledTouchSlop.toFloat()
                        if (hypot((event.rawX - downRawX).toDouble(), (event.rawY - downRawY).toDouble()) < slop) {
                            return true
                        }
                        dragging = true
                        onHandleGrab(kind, anchorX, (anchorTop + anchorBottom) * 0.5f)
                    }
                    if (dragging) {
                        onHandleMove(localX - grabOffsetX, localY - grabOffsetY)
                    }
                    return true
                }
                MotionEvent.ACTION_UP -> {
                    if (dragging) {
                        dragging = false
                        onHandleRelease()
                    } else if (kind == HANDLE_CARET) {
                        onCaretTap()
                    }
                    return true
                }
                MotionEvent.ACTION_CANCEL -> {
                    if (dragging) {
                        dragging = false
                        onHandleRelease()
                    }
                    return true
                }
                else -> return false
            }
        }
    }

    private class HandleView(
        context: Context,
        private val drawable: Drawable,
        private val hotspotRatio: Float,
    ) : View(context) {
        override fun onDraw(canvas: Canvas) {
            super.onDraw(canvas)
            val density = resources.displayMetrics.density.coerceAtLeast(1f)
            val dw = drawable.intrinsicWidth.takeIf { it > 0 } ?: (22f * density).roundToInt()
            val dh = drawable.intrinsicHeight.takeIf { it > 0 } ?: (22f * density).roundToInt()
            val winHotspot = width * hotspotRatio
            val drawHotspot = dw * hotspotRatio
            val left = (winHotspot - drawHotspot).roundToInt().coerceIn(0, (width - dw).coerceAtLeast(0))
            val top = 0
            drawable.setBounds(left, top, left + dw, top + dh)
            drawable.draw(canvas)
        }
    }

    companion object {
        private const val HANDLE_TARGET_DP: Float = 48f
        private const val HANDLE_START: Int = 0
        private const val HANDLE_END: Int = 1
        private const val HANDLE_CARET: Int = 2
        private const val ACTION_CUT: Int = 1 shl 0
        private const val ACTION_COPY: Int = 1 shl 1
        private const val ACTION_PASTE: Int = 1 shl 2
        private const val ACTION_SELECT_ALL: Int = 1 shl 3
        private const val TOOLBAR_DISMISSED: Int = 0
        private const val TOOLBAR_CUT: Int = 1
        private const val TOOLBAR_COPY: Int = 2
        private const val TOOLBAR_PASTE: Int = 3
        private const val TOOLBAR_SELECT_ALL: Int = 4
    }
}
