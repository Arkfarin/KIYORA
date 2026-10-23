package io.github.arkfarin.kiyora.nativebridge

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock

/** OS integration for the Rust PCM engine. No WebView audio and no second decoder. */
class PlaybackService : Service() {
    companion object {
        private const val CHANNEL = "kiyora.playback"
        private const val NOTIFICATION = 48151
    }
    private val main = Handler(Looper.getMainLooper())
    private lateinit var audio: AudioManager
    private lateinit var notifications: NotificationManager
    private lateinit var wake: PowerManager.WakeLock
    private var active: NativePlayback.Registration? = null
    internal var session: MediaSession? = null
        private set
    private var focusRequest: AudioFocusRequest? = null
    private var focusEpoch = Any()
    private var focused = false
    private var foreground = false
    private var receiverRegistered = false
    private var pausedAt: Long? = null
    private val pendingIntents = mutableListOf<PendingIntent>()
    private val watchdog = object : Runnable {
        override fun run() {
            val now = SystemClock.elapsedRealtime()
            val player = active
            if (player != null && ((focused && now - player.lastUpdateMs > 30_000) ||
                    pausedAt?.let { now - it > 5 * 60_000 } == true)) {
                NativePlayback.control(player, "stop")
                stopPlayback()
                return
            }
            main.postDelayed(this, 10_000)
        }
    }
    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            val registration = active ?: return
            if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY && NativePlayback.owns(registration, this@PlaybackService)) {
                NativePlayback.control(registration, "pause")
                pausePlayback()
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        audio = getSystemService(AudioManager::class.java)
        notifications = getSystemService(NotificationManager::class.java)
        wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "kiyora:playback").apply { setReferenceCounted(false) }
        notifications.createNotificationChannel(NotificationChannel(CHANNEL, "Music playback", NotificationManager.IMPORTANCE_LOW))
        val filter = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
        if (Build.VERSION.SDK_INT >= 33) registerReceiver(noisy, filter, Context.RECEIVER_NOT_EXPORTED)
        else registerReceiver(noisy, filter)
        receiverRegistered = true
        main.postDelayed(watchdog, 10_000)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val owner = intent?.getStringExtra(NativePlayback.OWNER)
        try {
            if (owner == null || !NativePlayback.started(this, owner, intent.action)) stopSelfResult(startId)
        } catch (_: Exception) {
            active?.let { NativePlayback.control(it, "stop") }
            stopPlayback()
        }
        // Android must not recreate an empty service without its Rust player.
        return START_NOT_STICKY
    }

    internal fun attach(registration: NativePlayback.Registration) {
        if (active != null && active !== registration) {
            releaseFocus()
            session?.release()
            session = null
            pendingIntents.forEach { it.cancel() }
            pendingIntents.clear()
        }
        active = registration
        if (session == null) {
            session = MediaSession(this, "kiyora").apply {
                setCallback(object : MediaSession.Callback() {
                    override fun onPlay() {
                        if (!NativePlayback.owns(registration, this@PlaybackService)) return
                        promote()
                        NativePlayback.control(registration, "resume")
                    }
                    override fun onPause() { NativePlayback.control(registration, "pause") }
                    override fun onStop() { NativePlayback.control(registration, "stop") }
                    override fun onSeekTo(pos: Long) {
                        if (pos >= 0) NativePlayback.control(registration, "seek", pos)
                    }
                }, main)
                isActive = true
            }
        }
        val metadata = registration.metadata
        session?.setMetadata(MediaMetadata.Builder()
            .putString(MediaMetadata.METADATA_KEY_TITLE, metadata.optString("title"))
            .putString(MediaMetadata.METADATA_KEY_ARTIST, metadata.optString("performer"))
            .putLong(MediaMetadata.METADATA_KEY_DURATION, metadata.optLong("duration_ms", 0))
            .build())
        updatePlayback(publishNotification = false)
    }

    internal fun promote() {
        val notification = notification()
        if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
        else startForeground(NOTIFICATION, notification)
        foreground = true
    }

    internal fun acquirePlaybackFocus(): Boolean {
        promote()
        if (!focused) {
            val registration = checkNotNull(active)
            val epoch = Any().also { focusEpoch = it }
            val attributes = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MUSIC).build()
            val listener = AudioManager.OnAudioFocusChangeListener { change ->
                // Binder may deliver an old request's callback after stop/resume
                // or a profile change. It must never pause the replacement.
                if (active === registration && focusEpoch === epoch &&
                    (change == AudioManager.AUDIOFOCUS_LOSS ||
                        change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT ||
                        change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK)) {
                    NativePlayback.control(registration, "pause")
                    pausePlayback()
                }
            }
            val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(attributes).setAcceptsDelayedFocusGain(false)
                .setWillPauseWhenDucked(true).setOnAudioFocusChangeListener(listener, main)
                .build().also { focusRequest = it }
            val result = audio.requestAudioFocus(request)
            focused = result == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        }
        if (focused) { pausedAt = null; renewWakeLock(); session?.isActive = true }
        return focused
    }

    internal fun updatePlayback(publishNotification: Boolean = true) {
        val state = active?.state ?: return
        val kind = state.optString("state", "idle")
        val code = when (kind) {
            "playing" -> PlaybackState.STATE_PLAYING
            "buffering" -> PlaybackState.STATE_BUFFERING
            "paused" -> PlaybackState.STATE_PAUSED
            "failed" -> PlaybackState.STATE_ERROR
            else -> PlaybackState.STATE_STOPPED
        }
        session?.setPlaybackState(PlaybackState.Builder()
            .setActions(PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE or PlaybackState.ACTION_STOP or PlaybackState.ACTION_SEEK_TO)
            .setState(code, state.optLong("position_ms", 0), if (kind == "playing") 1f else 0f, SystemClock.elapsedRealtime()).build())
        // Attaching for a notification command must not repeat pause/stop side
        // effects before that command has even reached the Rust player.
        if (!publishNotification) return
        when (kind) {
            "playing", "buffering" -> {
                if (focused) renewWakeLock()
                if (foreground) promote()
            }
            "paused" -> pausePlayback()
            "idle", "failed" -> stopPlayback()
        }
    }

    internal fun pausePlayback() {
        if (pausedAt == null) pausedAt = SystemClock.elapsedRealtime()
        releaseFocus()
        if (wake.isHeld) wake.release()
        active?.state?.put("state", "paused")
        session?.setPlaybackState(PlaybackState.Builder()
            .setActions(PlaybackState.ACTION_PLAY or PlaybackState.ACTION_STOP or PlaybackState.ACTION_SEEK_TO)
            .setState(PlaybackState.STATE_PAUSED, active?.state?.optLong("position_ms", 0) ?: 0, 0f, SystemClock.elapsedRealtime()).build())
        if (foreground) {
            // startForeground posts through ActivityManager asynchronously.
            // Publish the paused content through the same ordered path before
            // detaching; a direct notify() could be overwritten by an older
            // in-flight foreground notification carrying a Pause action.
            promote()
            stopForeground(STOP_FOREGROUND_DETACH)
        } else if (session != null) {
            notifications.notify(NOTIFICATION, notification())
        }
        foreground = false
    }

    internal fun stopPlayback() {
        NativePlayback.retiring(this)
        releaseFocus()
        if (wake.isHeld) wake.release()
        session?.isActive = false
        stopForeground(STOP_FOREGROUND_REMOVE)
        foreground = false
        notifications.cancel(NOTIFICATION)
        pendingIntents.forEach { it.cancel() }
        pendingIntents.clear()
        stopSelf()
    }

    private fun releaseFocus() {
        focusEpoch = Any()
        focusRequest?.let { audio.abandonAudioFocusRequest(it) }
        focusRequest = null
        focused = false
    }

    private fun renewWakeLock() {
        // Rust reports state once per second; the timeout bounds a dead bridge.
        if (!wake.isHeld) wake.acquire(10 * 60 * 1000L)
    }

    private fun action(name: String): PendingIntent {
        val owner = checkNotNull(active).owner
        val intent = Intent(this, PlaybackService::class.java).setAction(name)
            .setData(android.net.Uri.parse("kiyora://media/$owner/$name"))
            .putExtra(NativePlayback.OWNER, owner)
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val pending = if (name == NativePlayback.RESUME) PendingIntent.getForegroundService(this, 0, intent, flags)
        else PendingIntent.getService(this, 0, intent, flags)
        if (!pendingIntents.contains(pending)) pendingIntents.add(pending)
        return pending
    }

    @Suppress("DEPRECATION")
    private fun notification(): Notification {
        val registration = checkNotNull(active)
        val playing = registration.state.optString("state") in listOf("playing", "buffering")
        val builder = Notification.Builder(this, CHANNEL)
        if (Build.VERSION.SDK_INT >= 31) builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        builder.setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle(registration.metadata.optString("title", "kiyora"))
            .setContentText(registration.metadata.optString("performer"))
            .setCategory(Notification.CATEGORY_TRANSPORT).setOnlyAlertOnce(true)
            .setVisibility(Notification.VISIBILITY_PUBLIC).setOngoing(playing)
            .setDeleteIntent(action(NativePlayback.STOP))
            .addAction(if (playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play,
                if (playing) "Pause" else "Play", action(if (playing) NativePlayback.PAUSE else NativePlayback.RESUME))
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Stop", action(NativePlayback.STOP))
        session?.let { builder.setStyle(Notification.MediaStyle().setMediaSession(it.sessionToken).setShowActionsInCompactView(0, 1)) }
        packageManager.getLaunchIntentForPackage(packageName)?.let { launch ->
            builder.setContentIntent(PendingIntent.getActivity(this, 1, launch, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
        }
        return builder.build()
    }

    override fun onDestroy() {
        main.removeCallbacks(watchdog)
        releaseFocus()
        if (wake.isHeld) wake.release()
        if (receiverRegistered) unregisterReceiver(noisy)
        session?.release()
        session = null
        pendingIntents.forEach { it.cancel() }
        pendingIntents.clear()
        notifications.cancel(NOTIFICATION)
        NativePlayback.destroyed(this)
        active = null
        super.onDestroy()
    }
}
