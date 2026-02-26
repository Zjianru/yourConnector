package dev.yourconnector.mobile

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  private val logTag = "YC.PairLink"
  private var webView: WebView? = null
  private var pendingPairLink: String? = null
  private var pendingPairLinkRetryCount = 0
  private var pairLinkDeliveryInFlight = false

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    cachePairLink(intent)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    this.webView = webView
    flushPendingPairLink()
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    cachePairLink(intent)
    flushPendingPairLink()
  }

  override fun onResume() {
    super.onResume()
    flushPendingPairLink()
  }

  private fun cachePairLink(intent: Intent?) {
    val data = intent?.data ?: return
    if (!isPairingUri(data)) return
    pendingPairLink = data.toString()
    pendingPairLinkRetryCount = 0
    Log.i(logTag, "cached deep-link: $pendingPairLink")
  }

  private fun isPairingUri(uri: Uri): Boolean {
    val scheme = uri.scheme ?: return false
    val host = uri.host ?: return false
    return scheme.equals("yc", ignoreCase = true) && host.equals("pair", ignoreCase = true)
  }

  private fun flushPendingPairLink() {
    val rawUrl = pendingPairLink ?: return
    val targetWebView = webView ?: return
    if (pairLinkDeliveryInFlight) return
    pairLinkDeliveryInFlight = true
    val encoded = org.json.JSONObject.quote(rawUrl)
    val script = """
      (function(rawUrl) {
        if (typeof window.__YC_HANDLE_PAIR_LINK__ === "function") {
          window.__YC_HANDLE_PAIR_LINK__(rawUrl);
          return "delivered";
        }
        window.__YC_PENDING_PAIR_LINKS__ = window.__YC_PENDING_PAIR_LINKS__ || [];
        if (window.__YC_PENDING_PAIR_LINKS__.indexOf(rawUrl) === -1) {
          window.__YC_PENDING_PAIR_LINKS__.push(rawUrl);
        }
        return "queued";
      })($encoded);
    """.trimIndent()

    targetWebView.post {
      targetWebView.evaluateJavascript(script) { result ->
        pairLinkDeliveryInFlight = false
        if (result?.contains("delivered") == true) {
          Log.i(logTag, "delivered deep-link to webview")
          pendingPairLink = null
          pendingPairLinkRetryCount = 0
          return@evaluateJavascript
        }
        if (pendingPairLink == null) {
          pendingPairLinkRetryCount = 0
          return@evaluateJavascript
        }
        if (pendingPairLinkRetryCount >= 20) {
          Log.w(logTag, "deep-link delivery timed out after retries")
          pendingPairLinkRetryCount = 0
          return@evaluateJavascript
        }
        pendingPairLinkRetryCount += 1
        Log.d(logTag, "webview handler not ready, retry=$pendingPairLinkRetryCount")
        targetWebView.postDelayed({ flushPendingPairLink() }, 250)
      }
    }
  }
}
