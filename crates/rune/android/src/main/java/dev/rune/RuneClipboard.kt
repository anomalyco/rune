package dev.rune

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.os.PersistableBundle
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.util.concurrent.atomic.AtomicLong

/**
 * Process-wide wrapper around Android's [ClipboardManager] used by Rune's clipboard worker.
 *
 * Constructed on the main (UI) thread inside [RuneView] so [ClipboardManager] and its
 * [ClipboardManager.OnPrimaryClipChangedListener] bind to the main [android.os.Looper], while
 * reads and writes run off the UI thread on Rune's clipboard worker.
 */
class RuneClipboard private constructor(private val appContext: Context) {

    private val clipboard =
        appContext.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    private val lock = Any()
    private val revisionCounter = AtomicLong(1L)
    private var ownSeq: Long = 0L
    private var lastOwnSeq: Long = 0L
    private var clearedOwn: Boolean = false

    private val listener = ClipboardManager.OnPrimaryClipChangedListener {
        val desc = try {
            clipboard.primaryClipDescription
        } catch (_: RuntimeException) {
            null
        }
        val seenSeq = desc?.extras?.getLong(EXTRA_SEQ, 0L) ?: 0L
        var nextRev: Long? = null
        synchronized(lock) {
            if (seenSeq != 0L && seenSeq == lastOwnSeq) {
                return@OnPrimaryClipChangedListener
            }
            if (desc == null && clearedOwn) {
                clearedOwn = false
                return@OnPrimaryClipChangedListener
            }
            lastOwnSeq = 0L
            clearedOwn = false
            nextRev = revisionCounter.incrementAndGet()
        }
        nextRev?.let { notifyRevision(it) }
    }

    init {
        clipboard.addPrimaryClipChangedListener(listener)
        notifyRevision(revisionCounter.get())
    }

    fun revision(): Long = revisionCounter.get()

    fun writeText(text: String, sensitive: Boolean): String? {
        val label = if (sensitive) "secret" else "text"
        val err = stampAndSet(ClipData.newPlainText(label, text), sensitive)
        if (err == null) {
            RuneClipboardProvider.clear(appContext)
        }
        return err
    }

    fun writeHtml(text: String, html: String): String? {
        val err = stampAndSet(ClipData.newHtmlText("html", text, html), sensitive = false)
        if (err == null) {
            RuneClipboardProvider.clear(appContext)
        }
        return err
    }

    fun writeImage(png: ByteArray): String? {
        RuneClipboardProvider.checkDeclared(appContext)?.let { return it }
        val uri = try {
            RuneClipboardProvider.putImage(appContext, png)
        } catch (e: IOException) {
            return e.message ?: "failed to store clipboard image"
        } catch (e: RuntimeException) {
            return e.message ?: e.javaClass.simpleName
        }
        val clip = ClipData.newUri(appContext.contentResolver, "image", uri)
        val err = stampAndSet(clip, sensitive = false)
        if (err != null) {
            RuneClipboardProvider.clear(appContext)
        }
        return err
    }

    fun clear(): String? {
        return try {
            if (Build.VERSION.SDK_INT >= 28) {
                if (clipboard.hasPrimaryClip()) {
                    synchronized(lock) {
                        lastOwnSeq = 0L
                        clearedOwn = true
                    }
                    clipboard.clearPrimaryClip()
                    val next = synchronized(lock) { revisionCounter.incrementAndGet() }
                    notifyRevision(next)
                }
            } else {
                val err = stampAndSet(ClipData.newPlainText("", ""), sensitive = false)
                if (err != null) return err
            }
            RuneClipboardProvider.clear(appContext)
            null
        } catch (e: RuntimeException) {
            synchronized(lock) { clearedOwn = false }
            e.message ?: e.javaClass.simpleName
        }
    }

    fun readText(): String? {
        val clip = currentClip() ?: return null
        if (clip.itemCount <= 0) return null
        return extractText(clip, clip.getItemAt(0))
    }

    /**
     * Reads all requested forms from a single `primaryClip` snapshot and returns
     * `[text, html, png]`, or `null` when the clipboard holds nothing usable.
     */
    fun readContent(includeImage: Boolean): Array<Any?>? {
        val clip = currentClip() ?: return null
        if (clip.itemCount <= 0) return null
        val item = clip.getItemAt(0)
        val text = extractText(clip, item)
        val html = item.htmlText?.takeIf {
            it.isNotEmpty() && it.toByteArray(Charsets.UTF_8).size <= MAX_RICH_BYTES
        }
        val png = if (includeImage) extractImagePng(clip, item) else null
        if (text == null && html == null && png == null) return null
        return arrayOf(text, html, png)
    }

    private fun currentClip(): ClipData? {
        return try {
            if (!clipboard.hasPrimaryClip()) return null
            clipboard.primaryClip
        } catch (_: RuntimeException) {
            null
        }
    }

    private fun extractText(clip: ClipData, item: ClipData.Item): String? {
        item.text?.let { return it.toString() }
        val uri = item.uri ?: return null
        if (!hasMimePrefix(clip.description, uri, "text/")) return null
        return try {
            appContext.contentResolver.openInputStream(uri)?.use { stream ->
                readBounded(stream, MAX_RICH_BYTES)?.toString(Charsets.UTF_8)
            }
        } catch (_: Exception) {
            null
        }
    }

    private fun extractImagePng(clip: ClipData, item: ClipData.Item): ByteArray? {
        val uri = item.uri ?: return null
        if (!hasMimePrefix(clip.description, uri, "image/")) return null
        val raw = try {
            appContext.contentResolver.openInputStream(uri)?.use { stream ->
                readBounded(stream, MAX_IMAGE_BYTES)
            }
        } catch (_: Exception) {
            null
        } ?: return null
        if (raw.isEmpty()) return null
        if (isPng(raw)) return raw
        return decodeToPng(raw)
    }

    private fun hasMimePrefix(desc: ClipDescription?, uri: Uri, prefix: String): Boolean {
        val resolved = try {
            appContext.contentResolver.getType(uri)
        } catch (_: RuntimeException) {
            null
        }
        if (resolved != null && resolved.startsWith(prefix, ignoreCase = true)) {
            return true
        }
        if (desc != null) {
            for (i in 0 until desc.mimeTypeCount) {
                if (desc.getMimeType(i).startsWith(prefix, ignoreCase = true)) {
                    return true
                }
            }
        }
        return false
    }

    private fun stampAndSet(clip: ClipData, sensitive: Boolean): String? {
        val seq: Long
        synchronized(lock) {
            ownSeq += 1L
            seq = ownSeq
            lastOwnSeq = seq
            clearedOwn = false
        }
        val extras = clip.description.extras ?: PersistableBundle()
        extras.putLong(EXTRA_SEQ, seq)
        if (sensitive) {
            extras.putBoolean(EXTRA_IS_SENSITIVE, true)
        }
        clip.description.extras = extras
        return try {
            clipboard.setPrimaryClip(clip)
            val next = synchronized(lock) { revisionCounter.incrementAndGet() }
            notifyRevision(next)
            null
        } catch (e: RuntimeException) {
            synchronized(lock) {
                if (lastOwnSeq == seq) {
                    lastOwnSeq = 0L
                }
            }
            e.message ?: e.javaClass.simpleName
        }
    }

    private fun notifyRevision(revision: Long) {
        try {
            nativeOnRevisionChanged(revision)
        } catch (_: UnsatisfiedLinkError) {
            // Native library not bound yet or custom host without JNI registration.
        }
    }

    private external fun nativeOnRevisionChanged(revision: Long)

    companion object {
        private const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"
        private const val EXTRA_SEQ = "dev.rune.clip_seq"
        private const val MAX_RICH_BYTES = 4 * 1024 * 1024
        private const val MAX_IMAGE_BYTES = 32 * 1024 * 1024
        private const val MAX_IMAGE_PIXELS = 64L * 1024L * 1024L

        @Volatile
        private var instance: RuneClipboard? = null

        /** Returns the process-wide [RuneClipboard], initializing it on the calling (UI) thread. */
        fun getOrCreate(context: Context): RuneClipboard {
            instance?.let { return it }
            return synchronized(this) {
                instance ?: RuneClipboard(context.applicationContext).also { instance = it }
            }
        }

        private fun readBounded(stream: InputStream, maxBytes: Int): ByteArray? {
            val out = ByteArrayOutputStream()
            val buf = ByteArray(16 * 1024)
            var total = 0
            while (true) {
                val n = stream.read(buf)
                if (n <= 0) break
                total += n
                if (total > maxBytes) return null
                out.write(buf, 0, n)
            }
            return out.toByteArray()
        }

        private fun isPng(bytes: ByteArray): Boolean {
            if (bytes.size < 8) return false
            return bytes[0] == 0x89.toByte() &&
                bytes[1] == 0x50.toByte() &&
                bytes[2] == 0x4E.toByte() &&
                bytes[3] == 0x47.toByte() &&
                bytes[4] == 0x0D.toByte() &&
                bytes[5] == 0x0A.toByte() &&
                bytes[6] == 0x1A.toByte() &&
                bytes[7] == 0x0A.toByte()
        }

        private fun decodeToPng(raw: ByteArray): ByteArray? {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(raw, 0, raw.size, bounds)
            val w = bounds.outWidth.toLong()
            val h = bounds.outHeight.toLong()
            if (w <= 0L || h <= 0L || w * h > MAX_IMAGE_PIXELS) return null
            val bitmap = BitmapFactory.decodeByteArray(raw, 0, raw.size) ?: return null
            return try {
                val out = ByteArrayOutputStream()
                if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)) return null
                val png = out.toByteArray()
                if (png.size <= MAX_IMAGE_BYTES) png else null
            } finally {
                bitmap.recycle()
            }
        }
    }
}
