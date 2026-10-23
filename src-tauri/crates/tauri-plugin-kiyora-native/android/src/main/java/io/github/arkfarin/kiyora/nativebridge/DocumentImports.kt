package io.github.arkfarin.kiyora.nativebridge

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.io.FileOutputStream
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import android.os.CancellationSignal
import java.io.InputStream
import java.security.MessageDigest

internal class DocumentCopy {
    val cancelled = AtomicBoolean(false)
    val signal = CancellationSignal()
    val input = AtomicReference<InputStream?>(null)
    fun cancel() {
        cancelled.set(true); signal.cancel()
        try { input.getAndSet(null)?.close() } catch (_: Exception) { }
    }
}

/** Copy only explicitly granted content URIs. Names never select an output path. */
internal class DocumentImports(private val context: Context,
    directory: File = File(context.noBackupFilesDir, "kiyora-document-imports"),
    private val limit: Long = 8L * 1024 * 1024 * 1024) {
    // Android may expose the same private directory through /data/data and /data/user/0.
    private val root = directory.canonicalFile
    init {
        check(root.isDirectory || root.mkdirs())
        check(root.canonicalFile == root.absoluteFile)
        root.setReadable(false, false); root.setReadable(true, true)
        root.setWritable(false, false); root.setWritable(true, true)
        root.setExecutable(false, false); root.setExecutable(true, true)
        root.listFiles()?.filter { it.name.endsWith(".part") }?.forEach { check(it.delete()) }
    }
    @Synchronized
    fun stage(id: String, uriText: String, control: DocumentCopy): File {
        require(UUID.fromString(id).toString() == id)
        val uri = Uri.parse(uriText)
        require(uri.scheme == "content" && uri.authority != null && uriText.length <= 4096)
        check(!control.cancelled.get())
        val uriHash = MessageDigest.getInstance("SHA-256").digest(uriText.toByteArray()).joinToString("") { "%02x".format(it) }
        val existing = root.listFiles()?.filter { it.name.startsWith("$id-") && !it.name.endsWith(".part") }.orEmpty()
        if (existing.isNotEmpty()) {
            require(existing.size == 1 && existing[0].name.startsWith("$id-$uriHash."))
            check(existing[0].canonicalFile == existing[0].absoluteFile && existing[0].isFile)
            return existing[0]
        }
        val name = context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null, control.signal)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        } ?: throw IllegalArgumentException()
        val extension = name.substringAfterLast('.', "").lowercase(java.util.Locale.ROOT)
        require(extension in setOf("mp3", "flac", "m4a", "mp4", "aac", "ogg", "opus", "wav", "aiff", "aif"))
        val used = root.listFiles()?.sumOf { check(it.isFile && it.canonicalFile == it.absoluteFile); it.length() } ?: 0L
        val destination = File(root, "$id-$uriHash.$extension")
        val temporary = File(root, "$id-$uriHash.$extension.part")
        check(temporary.createNewFile())
        try {
            var total = 0L
            context.contentResolver.openAssetFileDescriptor(uri, "r", control.signal).use { descriptor ->
                requireNotNull(descriptor)
                val input = descriptor.createInputStream()
                control.input.set(input)
                check(!control.cancelled.get())
                FileOutputStream(temporary).use { output ->
                    val buffer = ByteArray(256 * 1024)
                    while (true) {
                        check(!control.cancelled.get())
                        val read = input.read(buffer)
                        if (read < 0) break
                        total = Math.addExact(total, read.toLong())
                        check(total <= limit - used)
                        output.write(buffer, 0, read)
                    }
                    check(total > 0 && !control.cancelled.get())
                    output.fd.sync()
                }
            }
            check(!control.cancelled.get() && !destination.exists() && temporary.renameTo(destination))
            val fd = android.system.Os.open(root.path, android.system.OsConstants.O_RDONLY, 0)
            try { android.system.Os.fsync(fd) } finally { android.system.Os.close(fd) }
            return destination
        } finally { control.input.getAndSet(null)?.close(); temporary.delete() }
    }
    @Synchronized
    fun release(id: String) {
        require(UUID.fromString(id).toString() == id)
        root.listFiles()?.filter { it.name.startsWith("$id-") }?.forEach {
            check(it.canonicalFile == it.absoluteFile && it.isFile)
            check(it.delete())
        }
        val fd = android.system.Os.open(root.path, android.system.OsConstants.O_RDONLY, 0)
        try { android.system.Os.fsync(fd) } finally { android.system.Os.close(fd) }
    }
}
