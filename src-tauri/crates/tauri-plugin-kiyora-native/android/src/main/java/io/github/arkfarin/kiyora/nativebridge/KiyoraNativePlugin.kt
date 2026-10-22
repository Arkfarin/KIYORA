package io.github.arkfarin.kiyora.nativebridge

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import org.json.JSONArray
import org.json.JSONObject

@TauriPlugin
class KiyoraNativePlugin(activity: Activity) : Plugin(activity) {
    private val context = activity.applicationContext
    private val secrets by lazy { KeystoreSecrets(context) }
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
    fun storagePaths(invoke: Invoke) = run(invoke) {
        val root = File(context.noBackupFilesDir, "kiyora")
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
}
