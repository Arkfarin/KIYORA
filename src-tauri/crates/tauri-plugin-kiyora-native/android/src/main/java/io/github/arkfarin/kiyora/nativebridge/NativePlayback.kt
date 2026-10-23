package io.github.arkfarin.kiyora.nativebridge

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import java.util.UUID
import org.json.JSONObject

/** All lifecycle transitions run on main. The owner fences delayed Rust calls
 * and notification intents across profile/player changes. No audio bytes cross IPC. */
internal object NativePlayback {
    const val OWNER = "kiyora.media.owner"
    const val RESUME = "kiyora.media.resume"
    const val PAUSE = "kiyora.media.pause"
    const val STOP = "kiyora.media.stop"
    private val main = Handler(Looper.getMainLooper())
    // Owns only applicationContext and the currently running service, released
    // by close()/destroyed(). Never retains an Activity or a WebView.
    @SuppressLint("StaticFieldLeak")
    private var registration: Registration? = null
    // stopSelf() is asynchronous. Never attach a replacement to an instance
    // that is already scheduled for destruction.
    @SuppressLint("StaticFieldLeak")
    private var retiringService: PlaybackService? = null

    internal class Registration(
        val owner: String,
        val context: Context,
        val send: (String, Long?) -> Unit,
    ) {
        var service: PlaybackService? = null
        var metadata = JSONObject()
        var state = JSONObject().put("state", "idle")
        var ready: ((Boolean) -> Unit)? = null
        var lastUpdateMs = SystemClock.elapsedRealtime()
    }

    fun open(context: Context, owner: String, send: (String, Long?) -> Unit) {
        checkMain()
        require(UUID.fromString(owner).toString() == owner)
        check(registration == null) { "media session already registered" }
        registration = Registration(owner, context.applicationContext, send)
    }

    fun requireOwner(owner: String): Registration {
        checkMain()
        return checkNotNull(registration?.takeIf { it.owner == owner })
    }

    fun owns(active: Registration, service: PlaybackService): Boolean {
        checkMain()
        return registration === active && active.service === service && retiringService !== service
    }

    fun prepare(owner: String, metadata: JSONObject, ready: (Boolean) -> Unit) {
        val active = requireOwner(owner)
        require(metadata.getString("title").length <= 1024)
        require(metadata.getString("performer").length <= 1024)
        require(metadata.getLong("duration_ms") >= 0)
        UUID.fromString(metadata.getString("track_id"))
        active.metadata = metadata
        active.lastUpdateMs = SystemClock.elapsedRealtime()
        active.state = JSONObject().put("state", "buffering").put("position_ms", 0)
        begin(active, ready)
    }

    fun resume(owner: String, ready: (Boolean) -> Unit) {
        val active = requireOwner(owner)
        check(active.metadata.has("track_id"))
        begin(active, ready)
    }

    private fun begin(active: Registration, ready: (Boolean) -> Unit) {
        check(active.ready == null)
        active.ready = ready
        active.lastUpdateMs = SystemClock.elapsedRealtime()
        if (retiringService == null) startService(active)
        main.postDelayed({
            if (registration === active && active.ready === ready) {
                active.ready = null
                active.service?.stopPlayback()
                ready(false)
            }
        }, 8_000)
    }

    private fun startService(active: Registration) {
        try {
            // Start/promote the service before requesting focus (required for
            // background playback on Android 15+). Resolve Rust only after focus.
            val intent = Intent(active.context, PlaybackService::class.java).putExtra(OWNER, active.owner)
            active.context.startForegroundService(intent)
        } catch (_: Exception) {
            val ready = active.ready
            active.ready = null
            ready?.invoke(false)
        }
    }

    fun started(service: PlaybackService, owner: String, action: String?): Boolean {
        checkMain()
        if (retiringService === service) {
            return false
        }
        val active = registration?.takeIf { it.owner == owner }
            ?: return registration?.service === service
        if (active.service != null && active.service !== service) {
            return false
        }
        active.service = service
        service.attach(active)
        when (action) {
            PAUSE -> control(active, "pause")
            STOP -> control(active, "stop")
            RESUME -> {
                // A foreground PendingIntent must publish its notification now,
                // before the Rust worker processes the resume control.
                service.promote()
                control(active, "resume")
            }
            else -> {
                val ready = active.ready ?: return false
                active.ready = null
                val success = try { service.acquirePlaybackFocus() } catch (_: Exception) { false }
                if (!success) service.pausePlayback()
                ready(success)
            }
        }
        return true
    }

    fun update(owner: String, state: JSONObject) {
        val active = requireOwner(owner)
        require(state.getString("state") in listOf("idle", "buffering", "playing", "paused", "failed"))
        if (state.has("track_id") && !state.isNull("track_id")) {
            check(state.getString("track_id") == active.metadata.getString("track_id"))
        }
        if (state.has("position_ms")) require(state.getLong("position_ms") >= 0)
        active.state = state
        active.lastUpdateMs = SystemClock.elapsedRealtime()
        active.service?.updatePlayback()
    }

    fun pause(owner: String) {
        val active = requireOwner(owner)
        active.state.put("state", "paused")
        active.service?.pausePlayback()
    }

    fun stop(owner: String) {
        val active = requireOwner(owner)
        active.state = JSONObject().put("state", "idle")
        active.ready?.invoke(false)
        active.ready = null
        active.service?.stopPlayback()
    }

    fun close(owner: String) {
        val active = requireOwner(owner)
        stop(owner)
        registration = null
        // Detach immediately; a new registration may precede onDestroy.
        active.service = null
    }

    fun control(active: Registration, action: String, positionMs: Long? = null) {
        checkMain()
        if (registration !== active) return
        try { active.send(action, positionMs) }
        catch (_: Exception) { active.service?.stopPlayback() }
    }

    fun destroyed(service: PlaybackService) {
        checkMain()
        if (retiringService === service) retiringService = null
        registration?.takeIf { it.service === service }?.let { active ->
            active.service = null
            active.ready?.invoke(false)
            active.ready = null
            control(active, "stop")
        }
        registration?.takeIf { it.service == null && it.ready != null }?.let { active ->
            main.post {
                if (registration === active && active.ready != null && retiringService == null) startService(active)
            }
        }
    }

    fun retiring(service: PlaybackService) {
        checkMain()
        retiringService = service
        registration?.takeIf { it.service === service }?.service = null
    }

    private fun checkMain() = check(Looper.myLooper() == Looper.getMainLooper())
}
