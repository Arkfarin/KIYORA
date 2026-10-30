package io.github.arkfarin.kiyora.nativebridge

import android.app.Activity
import android.os.Handler
import android.os.Looper
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import org.json.JSONArray
import org.json.JSONObject

@InvokeArg
internal class OpenMediaArgs {
    lateinit var owner: String
    lateinit var handler: Channel
}

@TauriPlugin
class KiyoraNativePlugin(activity: Activity) : Plugin(activity) {
    private val context = activity.applicationContext
    private val secrets by lazy { KeystoreSecrets(context) }
    private val documents by lazy { DocumentImports(context) }
    private val copies = java.util.concurrent.ConcurrentHashMap<String, DocumentCopy>()
    private val documentWorker = ThreadPoolExecutor(
        1, 1, 30, TimeUnit.SECONDS, ArrayBlockingQueue<Runnable>(16)
    ).apply { allowCoreThreadTimeOut(true) }
    private val main = Handler(Looper.getMainLooper())
    private val worker = ThreadPoolExecutor(
        1, 1, 30, TimeUnit.SECONDS, ArrayBlockingQueue<Runnable>(16)
    ).apply { allowCoreThreadTimeOut(true) }

    private fun run(invoke: Invoke, action: () -> Unit) {
        try {
            worker.execute {
                try {
                    action()
                } catch (_: Exception) {
                    // Neither exception text nor arguments are safe to log.
                    invoke.reject("OS secure storage is unavailable", "SECURE_STORAGE_UNAVAILABLE")
                }
            }
        } catch (_: Exception) {
            invoke.reject("OS secure storage is unavailable", "SECURE_STORAGE_UNAVAILABLE")
        }
    }

    @Command
    fun stageDocument(invoke: Invoke) {
        try {
            val id = invoke.getArgs().getString("op_id")
            val uri = invoke.getArgs().getString("uri")
            require(java.util.UUID.fromString(id).toString() == id)
            val control = DocumentCopy()
            check(copies.putIfAbsent(id, control) == null)
            try {
                documentWorker.execute {
                    try {
                        val path = documents.stage(id, uri, control)
                        invoke.resolve(JSObject().put("path", path.path))
                    } catch (_: Exception) {
                        invoke.reject("Document could not be read", if (control.cancelled.get()) "CANCELLED" else "STORAGE")
                    } finally { copies.remove(id, control) }
                }
            } catch (_: Exception) { copies.remove(id, control); invoke.reject("Import queue is full", "BACKPRESSURE") }
        } catch (_: Exception) { invoke.reject("Invalid document request", "INVALID_INPUT") }
    }

    @Command
    fun cancelDocument(invoke: Invoke) {
        try { copies[invoke.getArgs().getString("op_id")]?.cancel(); invoke.resolve() }
        catch (_: Exception) { invoke.reject("Invalid document request", "INVALID_INPUT") }
    }

    @Command
    fun releaseDocument(invoke: Invoke) {
        try {
            val id = invoke.getArgs().getString("op_id")
            documentWorker.execute {
                try { documents.release(id); invoke.resolve() }
                catch (_: Exception) { invoke.reject("Document cleanup failed", "STORAGE") }
            }
        } catch (_: Exception) { invoke.reject("Import queue is full", "BACKPRESSURE") }
    }

    @Command
    fun storagePaths(invoke: Invoke) = run(invoke) {
        val root = File(context.noBackupFilesDir.canonicalFile, "kiyora")
        check(root.isDirectory || root.mkdirs())
        check(root.canonicalFile == root.absoluteFile)
        val result = JSObject()
        result.put("profiles", root.path)
        result.put("tdjson", File(context.applicationInfo.nativeLibraryDir, "libtdjson.so").path)
        invoke.resolve(result)
    }

    @Command
    fun getSecret(invoke: Invoke) = run(invoke) {
        val value = secrets.get(invoke.getArgs().getString("key"))
        try {
            val result = JSObject()
            result.put("value", value?.let { JSONArray(it.map { byte -> byte.toInt() and 0xff }) } ?: JSONObject.NULL)
            invoke.resolve(result)
        } finally {
            value?.fill(0)
        }
    }

    @Command
    fun setSecret(invoke: Invoke) = run(invoke) {
        val args = invoke.getArgs()
        val array = args.getJSONArray("value")
        require(array.length() == 32)
        val value = ByteArray(32)
        try {
            for (index in value.indices) {
                val byte = array.get(index)
                require(byte is Int && byte in 0..255)
                value[index] = byte.toByte()
            }
            secrets.set(args.getString("key"), value)
            invoke.resolve()
        } finally {
            value.fill(0)
        }
    }

    private fun media(invoke: Invoke, action: () -> Unit) {
        main.post {
            try { action() }
            catch (_: Exception) { invoke.reject("Native playback is unavailable", "PLAYBACK_UNAVAILABLE") }
            catch (_: UnsatisfiedLinkError) { invoke.reject("Native audio context is unavailable", "PLAYBACK_UNAVAILABLE") }
        }
    }
    private fun ready(invoke: Invoke): (Boolean) -> Unit = { success ->
        if (success) invoke.resolve()
        else invoke.reject("Native playback is unavailable", "PLAYBACK_UNAVAILABLE")
    }

    @Command
    fun openMediaSession(invoke: Invoke) = media(invoke) {
        val args = invoke.parseArgs(OpenMediaArgs::class.java)
        NativePlayback.open(context, args.owner) { action, position ->
            val event = JSObject().put("action", action)
            if (position != null) event.put("position_ms", position)
            args.handler.send(event)
        }
        invoke.resolve()
    }

    @Command
    fun preparePlayback(invoke: Invoke) = media(invoke) {
        val args = invoke.getArgs()
        NativePlayback.prepare(args.getString("owner"), args.getJSONObject("metadata"), ready(invoke))
    }

    @Command
    fun resumeNativePlayback(invoke: Invoke) = media(invoke) {
        NativePlayback.resume(invoke.getArgs().getString("owner"), ready(invoke))
    }

    @Command
    fun pauseNativePlayback(invoke: Invoke) = media(invoke) {
        NativePlayback.pause(invoke.getArgs().getString("owner"))
        invoke.resolve()
    }

    @Command
    fun updateNativePlayback(invoke: Invoke) = media(invoke) {
        val args = invoke.getArgs()
        NativePlayback.update(args.getString("owner"), args.getJSONObject("state"))
        invoke.resolve()
    }

    @Command
    fun setNativeArtwork(invoke: Invoke) = media(invoke) {
        val args = invoke.getArgs()
        val active = NativePlayback.requireOwner(args.getString("owner"))
        val array = args.getJSONArray("bytes")
        require(array.length() <= 256 * 1024)
        val bytes = ByteArray(array.length()) { index ->
            val value = array.getInt(index)
            require(value in 0..255)
            value.toByte()
        }
        active.service?.setArtwork(args.getString("track_id"), bytes)
        invoke.resolve()
    }

    @Command
    fun stopNativePlayback(invoke: Invoke) = media(invoke) {
        NativePlayback.stop(invoke.getArgs().getString("owner"))
        invoke.resolve()
    }

    @Command
    fun closeMediaSession(invoke: Invoke) = media(invoke) {
        NativePlayback.close(invoke.getArgs().getString("owner"))
        invoke.resolve()
    }
}
