package dev.rune

import android.os.Build
import android.view.KeyEvent
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.ExtractedText
import android.view.inputmethod.ExtractedTextRequest
import android.view.inputmethod.SurroundingText

/**
 * [android.view.inputmethod.InputConnection] bridging Android soft keyboards (IME) to [RuneView].
 *
 * Runs in dummy mode (fullEditor = false) so Rune's Rust TextEdit remains the sole source of
 * truth while text queries (getTextBeforeCursor, getTextAfterCursor, getSelectedText,
 * getExtractedText, getSurroundingText) read from the focused paragraph window in UTF-16
 * offsets that never split a surrogate pair, and edit/recomposition methods forward synchronously
 * on the UI thread in event order.
 */
internal class RuneInputConnection(
    private val view: RuneView,
) : BaseInputConnection(view, false) {

    private var batchDepth: Int = 0

    override fun beginBatchEdit(): Boolean {
        batchDepth += 1
        return true
    }

    override fun endBatchEdit(): Boolean {
        if (batchDepth > 0) {
            batchDepth -= 1
        }
        return batchDepth > 0
    }

    override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence? {
        if (n < 0) return null
        return view.onImeGetTextBeforeCursor(n) ?: ""
    }

    override fun getTextAfterCursor(n: Int, flags: Int): CharSequence? {
        if (n < 0) return null
        return view.onImeGetTextAfterCursor(n) ?: ""
    }

    override fun getSelectedText(flags: Int): CharSequence? {
        return view.onImeGetSelectedText()
    }

    override fun getExtractedText(request: ExtractedTextRequest?, flags: Int): ExtractedText? {
        val text = view.onImeGetWindowText() ?: return null
        val sel = view.onImeGetSelection() ?: return null
        if (sel.size < 2) return null
        return ExtractedText().apply {
            this.text = text
            startOffset = 0
            partialStartOffset = -1
            partialEndOffset = -1
            selectionStart = sel[0]
            selectionEnd = sel[1]
        }
    }

    override fun getSurroundingText(
        beforeLength: Int,
        afterLength: Int,
        flags: Int,
    ): SurroundingText? {
        if (Build.VERSION.SDK_INT < 31) return null
        if (beforeLength < 0 || afterLength < 0) return null
        val before = view.onImeGetTextBeforeCursor(beforeLength) ?: return null
        val selected = view.onImeGetSelectedText() ?: ""
        val after = view.onImeGetTextAfterCursor(afterLength) ?: return null
        val sel = view.onImeGetSelection()
        val offset = if (sel != null && sel.isNotEmpty() && sel[0] >= before.length) {
            sel[0] - before.length
        } else {
            -1
        }
        return SurroundingText(
            before + selected + after,
            before.length,
            before.length + selected.length,
            offset,
        )
    }

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        view.onImeCommitText(text?.toString() ?: "")
        return true
    }

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        view.onImeSetComposingText(text?.toString() ?: "", newCursorPosition)
        return true
    }

    override fun setComposingRegion(start: Int, end: Int): Boolean {
        view.onImeSetComposingRegion(start, end)
        return true
    }

    override fun finishComposingText(): Boolean {
        view.onImeFinishComposingText()
        return true
    }

    override fun setSelection(start: Int, end: Int): Boolean {
        view.onImeSetSelection(start, end)
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
        view.onImeDeleteSurroundingText(beforeLength, afterLength)
        return true
    }

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean {
        view.onImeDeleteSurroundingTextInCodePoints(beforeLength, afterLength)
        return true
    }

    override fun sendKeyEvent(event: KeyEvent): Boolean {
        if (view.onImeKeyEvent(event)) {
            return true
        }
        return super.sendKeyEvent(event)
    }

    override fun performEditorAction(editorAction: Int): Boolean {
        return view.onImeEditorAction(editorAction)
    }

    override fun closeConnection() {
        batchDepth = 0
        super.closeConnection()
    }
}
