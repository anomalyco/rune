package dev.rune

import android.view.KeyEvent
import android.view.inputmethod.BaseInputConnection

/**
 * [android.view.inputmethod.InputConnection] bridging Android soft keyboards (IME) to [RuneView].
 *
 * Runs in dummy mode (`fullEditor = false`) so Rune's Rust `TextEdit` remains the sole source of
 * truth while `commitText`, `setComposingText`, `finishComposingText`, `deleteSurroundingText`,
 * `sendKeyEvent`, and `performEditorAction` forward synchronously on the UI thread in event order.
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

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        view.onImeCommitText(text?.toString() ?: "")
        return true
    }

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        view.onImeSetComposingText(text?.toString() ?: "", newCursorPosition)
        return true
    }

    override fun finishComposingText(): Boolean {
        view.onImeFinishComposingText()
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
        view.onImeDeleteSurroundingText(beforeLength, afterLength)
        return true
    }

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean {
        view.onImeDeleteSurroundingText(beforeLength, afterLength)
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
