package dev.rune

import android.content.ClipDescription
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
import java.util.UUID

/**
 * Unexported [ContentProvider] serving the single PNG image Rune last copied to the Android
 * clipboard via `content://<package>.rune.clipboard/image/<token>.png`.
 *
 * Apps that want `Tx::copy_image` / rich image copy on Android declare this provider in
 * `AndroidManifest.xml` with `android:exported="false"` and `android:grantUriPermissions="true"`
 * so only the pasting app receives Android's temporary read grant from `ClipboardManager`.
 * Apps that omit the declaration still get text, HTML, sensitive secret copy, and image paste,
 * and receive `CopyImageError::Failed` when attempting to copy an image.
 */
class RuneClipboardProvider : ContentProvider() {

    override fun onCreate(): Boolean = true

    override fun getType(uri: Uri): String? {
        return if (matchesCurrent(uri) != null) MIME_PNG else null
    }

    override fun getStreamTypes(uri: Uri, mimeTypeFilter: String): Array<String>? {
        if (matchesCurrent(uri) == null) return null
        return if (ClipDescription.compareMimeTypes(MIME_PNG, mimeTypeFilter)) {
            arrayOf(MIME_PNG)
        } else {
            null
        }
    }

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? {
        val stored = matchesCurrent(uri) ?: return null
        val cols = if (projection != null && projection.isNotEmpty()) {
            projection
        } else {
            DEFAULT_PROJECTION
        }
        val cursor = MatrixCursor(cols, 1)
        val row = arrayOfNulls<Any>(cols.size)
        for (i in cols.indices) {
            row[i] = when (cols[i]) {
                OpenableColumns.DISPLAY_NAME -> DISPLAY_NAME
                OpenableColumns.SIZE -> stored.size.toLong()
                else -> null
            }
        }
        cursor.addRow(row)
        return cursor
    }

    @Throws(FileNotFoundException::class)
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") {
            throw SecurityException("RuneClipboardProvider is read-only")
        }
        val stored = matchesCurrent(uri)
            ?: throw FileNotFoundException("No clipboard image for $uri")
        if (!stored.file.isFile) {
            throw FileNotFoundException("Clipboard image file missing for $uri")
        }
        return ParcelFileDescriptor.open(stored.file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? {
        throw UnsupportedOperationException("RuneClipboardProvider is read-only")
    }

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int {
        throw UnsupportedOperationException("RuneClipboardProvider is read-only")
    }

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int {
        throw UnsupportedOperationException("RuneClipboardProvider is read-only")
    }

    private fun matchesCurrent(uri: Uri): StoredImage? {
        val ctx = context ?: return null
        if (uri.authority != authority(ctx)) return null
        val segments = uri.pathSegments
        if (segments.size != 2 || segments[0] != PATH_IMAGE) return null
        val stored = current ?: return null
        return if (segments[1] == "${stored.token}.png") stored else null
    }

    private class StoredImage(
        val token: String,
        val file: File,
        val size: Int,
    )

    companion object {
        private const val AUTHORITY_SUFFIX = ".rune.clipboard"
        private const val CACHE_DIR_NAME = "rune-clipboard"
        private const val FILE_NAME = "current.png"
        private const val PATH_IMAGE = "image"
        private const val DISPLAY_NAME = "image.png"
        private const val MIME_PNG = "image/png"
        private val DEFAULT_PROJECTION = arrayOf(
            OpenableColumns.DISPLAY_NAME,
            OpenableColumns.SIZE,
        )

        private val lock = Any()
        @Volatile
        private var current: StoredImage? = null

        fun authority(context: Context): String = context.packageName + AUTHORITY_SUFFIX

        /**
         * Verifies the host manifest declares this provider for [authority] with
         * `grantUriPermissions="true"`. Returns `null` when usable, or an error description.
         */
        fun checkDeclared(context: Context): String? {
            val auth = authority(context)
            val info = if (Build.VERSION.SDK_INT >= 33) {
                context.packageManager.resolveContentProvider(
                    auth,
                    PackageManager.ComponentInfoFlags.of(0L),
                )
            } else {
                @Suppress("DEPRECATION")
                context.packageManager.resolveContentProvider(auth, 0)
            }
            if (info == null || info.packageName != context.packageName) {
                return "clipboard image provider ($auth) is not declared in AndroidManifest.xml"
            }
            if (!info.grantUriPermissions) {
                return "clipboard image provider ($auth) must set android:grantUriPermissions=\"true\""
            }
            return null
        }

        /** Writes [png] atomically into the app cache directory and returns its content URI. */
        @Throws(IOException::class)
        fun putImage(context: Context, png: ByteArray): Uri {
            val dir = File(context.cacheDir, CACHE_DIR_NAME)
            if (!dir.isDirectory && !dir.mkdirs()) {
                throw IOException("failed to create clipboard cache directory")
            }
            val token = UUID.randomUUID().toString()
            val tmp = File(dir, "tmp-$token.png")
            val target = File(dir, FILE_NAME)
            try {
                FileOutputStream(tmp).use { fos ->
                    fos.write(png)
                    fos.flush()
                }
                synchronized(lock) {
                    if (!tmp.renameTo(target)) {
                        tmp.copyTo(target, overwrite = true)
                        tmp.delete()
                    }
                    current = StoredImage(token, target, png.size)
                }
            } catch (e: IOException) {
                tmp.delete()
                throw e
            }
            return Uri.Builder()
                .scheme("content")
                .authority(authority(context))
                .appendPath(PATH_IMAGE)
                .appendPath("$token.png")
                .build()
        }

        /** Drops the active token and deletes any cached PNG file. */
        fun clear(context: Context) {
            synchronized(lock) {
                current = null
                val dir = File(context.cacheDir, CACHE_DIR_NAME)
                File(dir, FILE_NAME).delete()
            }
        }
    }
}
