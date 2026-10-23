package io.github.arkfarin.kiyora.nativebridge

import android.app.Activity
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.session.MediaController
import android.media.session.PlaybackState
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

class PlaybackTestActivity : Activity()

/** Real OS service/session/focus/notifications. Rust audio is tested separately;
 * this package never connects to Telegram or reads production profiles. */
@RunWith(AndroidJUnit4::class)
class PlaybackServiceTest {
    private val instrument = InstrumentationRegistry.getInstrumentation()
    private val context = instrument.targetContext
    private val owner = UUID.randomUUID().toString()
    private val track = UUID.randomUUID().toString()
    private val controls = LinkedBlockingQueue<Pair<String, Long?>>()
    private val controlOrigins = LinkedBlockingQueue<String>()
    private lateinit var activity: ActivityScenario<PlaybackTestActivity>
    private fun main(action: () -> Unit) = instrument.runOnMainSync(action)
    private fun eventually(description: String, condition: () -> Boolean) {
        val deadline = SystemClock.elapsedRealtime() + 5_000
        while (!condition()) {
            assertTrue(description, SystemClock.elapsedRealtime() < deadline)
            // NotificationManager runs in another process; flushing the app's
            // main looper alone does not acknowledge notification publication.
            SystemClock.sleep(25)
        }
    }

    @Before fun open() {
        activity = ActivityScenario.launch(Intent(context, PlaybackTestActivity::class.java))
        main {
            NativePlayback.open(context, owner) { action, position ->
                controlOrigins.offer("$action: " + Throwable().stackTrace.take(8).joinToString("\n"))
                controls.offer(action to position)
            }
        }
    }

    @After fun close() {
        main { runCatching { NativePlayback.close(owner) } }
        instrument.waitForIdleSync()
        activity.close()
    }

    private fun prepare(): MediaController {
        val ready = CountDownLatch(1)
        var success = false
        main {
            NativePlayback.prepare(owner, JSONObject().put("track_id", track).put("title", "Instrumented song")
                .put("performer", "Test artist").put("duration_ms", 60_000)) { result ->
                success = result
                ready.countDown()
            }
        }
        assertTrue("service must resolve startup", ready.await(10, TimeUnit.SECONDS))
        assertTrue("foreground service must obtain audio focus", success)
        lateinit var controller: MediaController
        main {
            controller = MediaController(context, NativePlayback.requireOwner(owner).service!!.session!!.sessionToken)
            NativePlayback.update(owner, JSONObject().put("state", "playing").put("track_id", track).put("position_ms", 1234).put("duration_ms", 60_000))
        }
        return controller
    }

    @Test fun sessionMetadataNotificationTransportAndStop() {
        val controller = prepare()
        assertEquals("Instrumented song", controller.metadata?.getString(android.media.MediaMetadata.METADATA_KEY_TITLE))
        assertEquals(PlaybackState.STATE_PLAYING, controller.playbackState?.state)
        val notifications = context.getSystemService(NotificationManager::class.java)
        eventually("playing notification appears") { notifications.activeNotifications.any { it.notification.extras.getString("android.title") == "Instrumented song" } }
        controller.transportControls.pause()
        assertEquals("pause", controls.poll(3, TimeUnit.SECONDS)?.first)
        main { NativePlayback.pause(owner); NativePlayback.update(owner, JSONObject().put("state", "paused").put("track_id", track).put("position_ms", 1234)) }
        assertEquals(PlaybackState.STATE_PAUSED, controller.playbackState?.state)
        controller.transportControls.seekTo(3456)
        assertEquals("seek" to 3456L, controls.poll(3, TimeUnit.SECONDS))
        controller.transportControls.play()
        assertEquals("resume", controls.poll(3, TimeUnit.SECONDS)?.first)
        controller.transportControls.stop()
        assertEquals("stop", controls.poll(3, TimeUnit.SECONDS)?.first)
        main { NativePlayback.stop(owner) }
        instrument.waitForIdleSync()
        eventually("stop removes notification") { notifications.activeNotifications.isEmpty() }
    }

    @Test fun staleOwnersAndStaleTrackUpdatesCannotControlNewPlayback() {
        val controller = prepare()
        main {
            assertThrows(IllegalStateException::class.java) { NativePlayback.pause(UUID.randomUUID().toString()) }
            assertThrows(IllegalStateException::class.java) { NativePlayback.update(owner, JSONObject().put("state", "playing").put("track_id", UUID.randomUUID().toString())) }
        }
        assertEquals(PlaybackState.STATE_PLAYING, controller.playbackState?.state)
    }

    @Test fun audioFocusLossRequestsPauseAndReleasesForegroundPlayback() {
        prepare()
        val audio = context.getSystemService(AudioManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            val competing = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build())
                .setOnAudioFocusChangeListener({}, Handler(Looper.getMainLooper())).build()
            try {
                main { assertEquals(AudioManager.AUDIOFOCUS_REQUEST_GRANTED, audio.requestAudioFocus(competing)) }
                assertEquals("pause", controls.poll(3, TimeUnit.SECONDS)?.first)
                main { assertEquals("paused", NativePlayback.requireOwner(owner).state.getString("state")) }
            } finally { main { audio.abandonAudioFocusRequest(competing) } }
        }
    }

    @Test fun notificationControlsResumeInBackgroundAndAreRevokedOnClose() {
        prepare()
        val notifications = context.getSystemService(NotificationManager::class.java)
        eventually("pause action appears") { notifications.activeNotifications.singleOrNull()?.notification?.actions?.get(0)?.title == "Pause" }
        val pause = notifications.activeNotifications.single().notification.actions[0].actionIntent
        pause.send()
        assertEquals("pause", controls.poll(3, TimeUnit.SECONDS)?.first)
        main { NativePlayback.pause(owner) }
        eventually("resume action appears") { notifications.activeNotifications.singleOrNull()?.notification?.actions?.get(0)?.title == "Play" }
        val resume = notifications.activeNotifications.single().notification.actions[0].actionIntent
        activity.moveToState(Lifecycle.State.CREATED)
        resume.send()
        val resumedAction = controls.poll(3, TimeUnit.SECONDS)?.first
        assertEquals(controlOrigins.joinToString("\n"), "resume", resumedAction)
        val ready = CountDownLatch(1)
        var focused = false
        main { NativePlayback.resume(owner) { focused = it; ready.countDown() } }
        assertTrue(ready.await(10, TimeUnit.SECONDS))
        assertTrue("foreground playback must reacquire focus in background", focused)
        main { NativePlayback.close(owner) }
        instrument.waitForIdleSync()
        assertThrows(PendingIntent.CanceledException::class.java) { resume.send() }
        eventually("close removes notification") { notifications.activeNotifications.isEmpty() }
    }

    @Test fun missingRustHeartbeatStopsForegroundService() {
        prepare()
        main { NativePlayback.requireOwner(owner).lastUpdateMs = SystemClock.elapsedRealtime() - 31_000 }
        assertEquals("stop", controls.poll(12, TimeUnit.SECONDS)?.first)
        instrument.waitForIdleSync()
        eventually("watchdog removes notification") { context.getSystemService(NotificationManager::class.java).activeNotifications.isEmpty() }
    }

    @Test fun immediatePlayerReplacementRejectsLateOldControls() {
        val oldController = prepare()
        val nextOwner = UUID.randomUUID().toString()
        val nextControls = LinkedBlockingQueue<Pair<String, Long?>>()
        val ready = CountDownLatch(1)
        var focused = false
        try {
            main {
                NativePlayback.close(owner)
                NativePlayback.open(context, nextOwner) { action, position -> nextControls.offer(action to position) }
                NativePlayback.prepare(nextOwner, JSONObject().put("track_id", track).put("title", "Replacement")
                    .put("performer", "Test").put("duration_ms", 60_000)) {
                    focused = it; ready.countDown()
                }
            }
            assertTrue(ready.await(10, TimeUnit.SECONDS))
            assertTrue(focused)
            main { NativePlayback.update(nextOwner, JSONObject().put("state", "playing").put("track_id", track).put("position_ms", 0)) }
            oldController.transportControls.stop()
            oldController.transportControls.play()
            context.startService(Intent(context, PlaybackService::class.java)
                .setAction(NativePlayback.PAUSE).putExtra(NativePlayback.OWNER, owner))
            assertNull("old session must not control replacement", nextControls.poll(1, TimeUnit.SECONDS))
            main {
                val player = NativePlayback.requireOwner(nextOwner)
                assertEquals("playing", player.state.getString("state"))
                assertTrue(player.service!!.session!!.isActive)
            }
        } finally { main { runCatching { NativePlayback.close(nextOwner) } } }
    }
}
