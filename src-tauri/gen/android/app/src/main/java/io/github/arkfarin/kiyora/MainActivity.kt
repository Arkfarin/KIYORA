package io.github.arkfarin.kiyora

import android.media.AudioManager
import android.os.Bundle
import android.view.View
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  override val handleBackNavigation = false

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Select the media stream for hardware buttons; never set the OS volume.
    volumeControlStream = AudioManager.STREAM_MUSIC
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    val content = findViewById<View>(android.R.id.content)
    val bars = WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
    val handled = bars or WindowInsetsCompat.Type.ime()
    // Native padding works on older WebViews too. Zero handled insets before
    // dispatch to avoid CSS safe-area / IME double padding on newer WebViews.
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val safe = insets.getInsets(bars)
      val keyboard = insets.getInsets(WindowInsetsCompat.Type.ime())
      view.setPadding(safe.left, safe.top, safe.right, maxOf(safe.bottom, keyboard.bottom))
      WindowInsetsCompat.Builder(insets).setInsets(handled, Insets.NONE).build()
    }
    ViewCompat.requestApplyInsets(content)
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        if (ViewCompat.getRootWindowInsets(content)?.isVisible(WindowInsetsCompat.Type.ime()) == true) {
          WindowInsetsControllerCompat(window, webView).hide(WindowInsetsCompat.Type.ime())
          return
        }
        webView.evaluateJavascript("window.dispatchEvent(new Event('kiyora:back', {cancelable:true}))") { unhandled ->
          // Root Back backgrounds the task, preserving the process-owned player.
          if (unhandled == "true") moveTaskToBack(true)
        }
      }
    })
  }
}
