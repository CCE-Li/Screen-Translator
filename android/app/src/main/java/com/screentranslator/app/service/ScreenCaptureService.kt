package com.screentranslator.app.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Looper
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.chinese.ChineseTextRecognizerOptions
import com.google.mlkit.vision.text.japanese.JapaneseTextRecognizerOptions
import com.google.mlkit.vision.text.latin.TextRecognizerOptions
import com.screentranslator.app.MainActivity
import com.screentranslator.app.SettingsStore
import com.screentranslator.app.capture.ScreenCapturer
import com.screentranslator.app.core.Frame
import com.screentranslator.app.core.OverlayText
import com.screentranslator.app.core.RectF
import com.screentranslator.app.core.RegionSession
import com.screentranslator.app.core.Translation
import com.screentranslator.app.core.TranslationTask
import com.screentranslator.app.core.UpdateResult
import com.screentranslator.app.ocr.MlKitOcrProvider
import com.screentranslator.app.overlay.TranslationOverlay
import com.screentranslator.app.translation.LocalTranslationProvider
import com.screentranslator.app.translation.OpenAiTranslationProvider
import com.screentranslator.app.translation.TranslateRequest
import com.screentranslator.app.translation.TranslationProvider
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Foreground service running capture → OCR → translate → overlay.
 *
 * The [RegionSession] is owned by the capture loop thread. A single translator
 * thread performs the network calls and posts results back through a blocking
 * queue, so the session is never mutated from two threads at once.
 */
class ScreenCaptureService : Service() {

    companion object {
        const val EXTRA_RESULT_CODE = "result_code"
        const val EXTRA_RESULT_DATA = "result_data"
        const val ACTION_STOP = "com.screentranslator.app.STOP"
        private const val CHANNEL_ID = "screen_translator"
        private const val NOTIFICATION_ID = 1
        private const val MAX_CAPTURE_DIM = 720
        private const val CAPTURE_FPS = 5
        private const val BATCH_SIZE = 8
    }

    private val running = AtomicBoolean(false)
    private var mediaProjection: MediaProjection? = null
    private var projectionCallback: MediaProjection.Callback? = null
    private var capturer: ScreenCapturer? = null
    private var overlay: TranslationOverlay? = null
    private lateinit var session: RegionSession
    private lateinit var translator: TranslationProvider
    private lateinit var settings: SettingsStore

    private val captureThread = HandlerThread("capture")
    private val translatorQueue = LinkedBlockingQueue<TranslationTask>()
    private val doneQueue = LinkedBlockingQueue<Pair<TranslationTask, List<Translation>?>>()
    private val mainHandler = Handler(Looper.getMainLooper())

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        settings = SettingsStore(this)
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopPipeline()
            return START_NOT_STICKY
        }
        val resultCode = intent?.getIntExtra(EXTRA_RESULT_CODE, 0) ?: 0
        @Suppress("DEPRECATION")
        val data = intent?.getParcelableExtra<Intent>(EXTRA_RESULT_DATA)
        if (resultCode == 0 || data == null) {
            android.util.Log.w("ScreenCaptureService", "missing media projection result")
            stopSelf()
            return START_NOT_STICKY
        }
        startForegroundCompat()
        startPipeline(resultCode, data)
        return START_NOT_STICKY
    }

    private fun startForegroundCompat() {
        val pi = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val stopPi = PendingIntent.getService(
            this, 1,
            Intent(this, ScreenCaptureService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val stopAction = Notification.Action.Builder(
            android.graphics.drawable.Icon.createWithResource(this, android.R.drawable.ic_menu_close_clear_cancel),
            "停止",
            stopPi,
        ).build()
        val notification = Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("Screen Translator")
            .setContentText("正在实时翻译选中区域…")
            .setSmallIcon(android.R.drawable.ic_menu_edit)
            .setContentIntent(pi)
            .setOngoing(true)
            .addAction(stopAction)
            .build()
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(
                NOTIFICATION_ID, notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun startPipeline(resultCode: Int, data: Intent) {
        val mpm = getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
        val projection = mpm.getMediaProjection(resultCode, data)
        if (projection == null) {
            android.util.Log.e("ScreenCaptureService", "no media projection available")
            stopSelf()
            return
        }
        mediaProjection = projection
        val callback = object : MediaProjection.Callback() {
            override fun onStop() {
                running.set(false)
            }
        }
        projectionCallback = callback
        projection.registerCallback(callback, mainHandler)

        val dm = resources.displayMetrics
        val cap = ScreenCapturer(projection, dm, MAX_CAPTURE_DIM)
        capturer = cap
        overlay = TranslationOverlay(this)

        // OCR engine per configured language.
        val ocrLang = settings.ocrLang
        val recognizer = when (ocrLang.lowercase()) {
            "zh" -> TextRecognition.getClient(ChineseTextRecognizerOptions.Builder().build())
            "ja" -> TextRecognition.getClient(JapaneseTextRecognizerOptions.Builder().build())
            else -> TextRecognition.getClient(TextRecognizerOptions.DEFAULT_OPTIONS)
        }
        val ocr = MlKitOcrProvider(recognizer, if (ocrLang.isEmpty()) "latin" else ocrLang)

        // Translator per configured provider.
        translator = if (settings.provider == "openai") {
            OpenAiTranslationProvider(
                baseUrl = settings.baseUrl,
                apiKey = settings.apiKey,
                model = settings.model,
            )
        } else {
            LocalTranslationProvider()
        }

        val dmCopy = dm
        val region = settings.region()
            ?: RectF(0f, 0f, dmCopy.widthPixels.toFloat(), dmCopy.heightPixels.toFloat())
        session = RegionSession(
            id = "r1",
            ocr = ocr,
            ocrCooldownMs = 600,
            ocrRetriggerMs = 3000,
            diffThreshold = 0.01f,
            sourceLang = if (ocrLang.isEmpty()) null else ocrLang,
            targetLang = settings.targetLang,
            cacheCapacity = 2048,
        )

        // Translator thread.
        val translatorThread = Thread {
            while (running.get() || translatorQueue.isNotEmpty()) {
                val task = try {
                    translatorQueue.poll(300, TimeUnit.MILLISECONDS) ?: continue
                } catch (e: InterruptedException) {
                    break
                }
                try {
                    val results = translator.translate(TranslateRequest(task.texts, task.sourceLang, task.targetLang))
                    doneQueue.put(task to results)
                } catch (e: Exception) {
                    android.util.Log.w("ScreenCaptureService", "translate failed: ${e.message}")
                    doneQueue.put(task to null)
                }
            }
        }
        translatorThread.name = "translator"
        translatorThread.start()

        captureThread.start()
        val captureHandler = Handler(captureThread.looper)

        running.set(true)
        cap.start()

        val intervalMs = (1000 / CAPTURE_FPS).toLong()
        val scale = cap.scale

        captureHandler.post(object : Runnable {
            private var lastStats = System.currentTimeMillis()
            override fun run() {
                if (!running.get()) return

                // Apply finished translations.
                var redraw = false
                while (true) {
                    val done = doneQueue.poll() ?: break
                    if (done.second == null) {
                        session.noteFailure(done.first)
                    } else {
                        session.acceptTranslations(done.first, done.second!!)
                        redraw = true
                    }
                }
                if (redraw) postOverlay(session.refreshOverlays(), scale, region)

                val frame = cap.acquireFrame()
                if (frame != null) {
                    val regionFrame = cropRegion(frame, region, scale)
                    if (regionFrame != null) {
                        when (val res = session.processFrame(regionFrame, System.currentTimeMillis())) {
                            is UpdateResult.Redraw -> postOverlay(res.overlays, scale, region)
                            UpdateResult.NoChange -> {}
                        }
                    }
                    for (task in session.drainRequests()) {
                        for (chunk in task.texts.chunked(BATCH_SIZE)) {
                            translatorQueue.put(task.copy(texts = chunk))
                        }
                    }
                }

                if (System.currentTimeMillis() - lastStats > 10_000) {
                    val d = session.takeStatsDelta()
                    android.util.Log.i(
                        "ScreenCaptureService",
                        "stats: ocr_runs=${d.ocrRuns} frames=${d.framesSeen} ocr_ms=${d.ocrMs} " +
                            "tc_hit=${"%.2f".format(session.translationCacheHitRate())} pending=${d.pending}"
                    )
                    session.resetStats()
                    lastStats = System.currentTimeMillis()
                }

                captureHandler.postDelayed(this, intervalMs)
            }
        })
    }

    private fun cropRegion(frame: Frame, region: RectF, scale: Float): Frame? {
        val sx = (region.x * scale).toInt().coerceAtLeast(0)
        val sy = (region.y * scale).toInt().coerceAtLeast(0)
        val sw = (region.width * scale).toInt().coerceAtLeast(1)
        val sh = (region.height * scale).toInt().coerceAtLeast(1)
        val ex = (sx + sw).coerceAtMost(frame.width)
        val ey = (sy + sh).coerceAtMost(frame.height)
        if (ex <= sx || ey <= sy) return null
        val w = ex - sx
        val h = ey - sy
        val out = IntArray(w * h)
        for (y in 0 until h) {
            val srcRow = (sy + y) * frame.stride
            val dstRow = y * w
            System.arraycopy(frame.pixels, srcRow + sx, out, dstRow, w)
        }
        return Frame(w, h, out, frame.timestamp)
    }

    private fun postOverlay(overlays: List<OverlayText>, scale: Float, region: RectF) {
        val screen = overlays.map {
            OverlayText(
                it.text,
                RectF(
                    region.x + it.box.x / scale,
                    region.y + it.box.y / scale,
                    it.box.width / scale,
                    it.box.height / scale,
                ),
                it.fontSize / scale,
                it.align,
            )
        }
        mainHandler.post { overlay?.show(screen) }
    }

    private fun stopPipeline() {
        running.set(false)
        capturer?.stop()
        mediaProjection?.let {
            projectionCallback?.let { cb -> it.unregisterCallback(cb) }
            it.stop()
        }
        mainHandler.post { overlay?.dismiss() }
        overlay = null
        captureThread.quitSafely()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun createNotificationChannel() {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Screen Translator", NotificationManager.IMPORTANCE_LOW)
        )
    }
}
