package io.github.arkfarin.kiyora

import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Real installed app/JNI/IPC check. Uses no credentials and changes no profiles. */
@RunWith(AndroidJUnit4::class)
class NativePackagingTest {
    @Test
    fun installedTdlibLoadsAndRustIpcResponds() {
        assertEquals(
            "Use scripts/check-android-app.sh: Tauri exits the process when its Activity is destroyed",
            "false",
            InstrumentationRegistry.getArguments().getString("waitForActivitiesToComplete")
        )
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val library = File(context.applicationInfo.nativeLibraryDir, "libtdjson.so")
        assertTrue("TDLib must be extracted into nativeLibraryDir", library.isFile)
        val digest = MessageDigest.getInstance("SHA-256")
        library.inputStream().use { input ->
            val buffer = ByteArray(65536)
            while (true) {
                val length = input.read(buffer)
                if (length < 0) break
                digest.update(buffer, 0, length)
            }
        }
        val actual = digest.digest().joinToString("") { "%02x".format(it.toInt() and 0xff) }
        val manifests = context.assets.list("native-licenses")!!.filter { it.startsWith("tdlib-") && it.endsWith(".json") }
        assertTrue("Installed TDLib must match a packaged ABI manifest", manifests.any {
            val record = context.assets.open("native-licenses/$it").bufferedReader().use { reader -> JSONObject(reader.readText()) }
            record.getString("sha256") == actual && record.getString("tdlib_version") == "1.8.69"
        })
        // Exercise Android's loader with the same runtime location used by Rust.
        // Missing libc++/OpenSSL/other native dependencies fail here on the device.
        System.load(library.absolutePath)
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        try {
            awaitJavascript(scenario, "Boolean(window.__TAURI_INTERNALS__)", "true")
            javascript(scenario, """
                window.__kiyoraPackagingProbe = null;
                window.__TAURI_INTERNALS__.invoke('backend_list_profiles')
                  .then(value => { window.__kiyoraPackagingProbe = Array.isArray(value) ? 'ok' : 'bad-response'; })
                  .catch(() => { window.__kiyoraPackagingProbe = 'ipc-error'; });
            """.trimIndent())
            awaitJavascript(scenario, "window.__kiyoraPackagingProbe", "\"ok\"")
        } finally {
            // Stop the Activity, but let Instrumentation.finish report the test
            // result before killing the process. Destroying the last Tauri
            // Activity here races JUnit reporting with Tao's process::exit(0).
            scenario.moveToState(androidx.lifecycle.Lifecycle.State.CREATED)
        }
    }

    private fun awaitJavascript(scenario: ActivityScenario<MainActivity>, expression: String, expected: String) {
        val deadline = SystemClock.elapsedRealtime() + 15000
        var result: String? = null
        while (SystemClock.elapsedRealtime() < deadline) {
            result = javascript(scenario, expression)
            if (result == expected) return
            SystemClock.sleep(50)
        }
        assertEquals("Packaged WebView/Rust IPC did not become ready", expected, result)
    }

    private fun javascript(scenario: ActivityScenario<MainActivity>, expression: String): String? {
        val done = CountDownLatch(1)
        val result = AtomicReference<String?>(null)
        scenario.onActivity { activity ->
            val webview = findWebView(activity.window.decorView)
            if (webview == null) done.countDown()
            else webview.evaluateJavascript(expression) { value ->
                result.set(value)
                done.countDown()
            }
        }
        assertTrue("WebView evaluation callback timed out", done.await(3, TimeUnit.SECONDS))
        return result.get()
    }

    private fun findWebView(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                findWebView(view.getChildAt(index))?.let { return it }
            }
        }
        return null
    }
}
