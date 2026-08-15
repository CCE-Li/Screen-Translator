package com.screentranslator.app.capture

import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.util.DisplayMetrics
import com.screentranslator.app.core.Frame

/**
 * Captures the screen via MediaProjection + VirtualDisplay + ImageReader at a
 * reduced resolution (longest side <= [maxDim]) to keep CPU/power low.
 */
class ScreenCapturer(
    private val mediaProjection: MediaProjection,
    private val displayMetrics: DisplayMetrics,
    maxDim: Int = 720,
) {
    private var imageReader: ImageReader? = null
    private var virtualDisplay: VirtualDisplay? = null

    /** 1.0 when capturing at native resolution, < 1.0 when downscaled. */
    val scale: Float =
        maxDim.toFloat() / maxOf(displayMetrics.widthPixels, displayMetrics.heightPixels)
            .toFloat().coerceAtLeast(1f)

    fun start() {
        val width = displayMetrics.widthPixels
        val height = displayMetrics.heightPixels
        val captureWidth = (width * scale).toInt().coerceAtLeast(1)
        val captureHeight = (height * scale).toInt().coerceAtLeast(1)
        imageReader = ImageReader.newInstance(
            captureWidth, captureHeight, PixelFormat.RGBA_8888, 2
        )
        virtualDisplay = mediaProjection.createVirtualDisplay(
            "ScreenTranslatorCapture",
            captureWidth,
            captureHeight,
            displayMetrics.densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
            imageReader?.surface,
            null,
            null,
        )
    }

    /**
     * Grab the latest frame (capture coordinates). Must be called on the
     * capture thread. Returns null when no new frame is available.
     */
    fun acquireFrame(): Frame? {
        val reader = imageReader ?: return null
        val image = reader.acquireLatestImage() ?: return null
        try {
            val w = image.width
            val h = image.height
            if (w <= 0 || h <= 0) return null
            val plane = image.planes[0]
            val buffer = plane.buffer
            val rowStride = plane.rowStride
            val pixelStride = plane.pixelStride
            val pixels = IntArray(w * h)
            buffer.rewind()
            for (row in 0 until h) {
                var pos = row * rowStride
                var idx = row * w
                for (col in 0 until w) {
                    val r = buffer.get(pos).toInt() and 0xFF
                    val g = buffer.get(pos + 1).toInt() and 0xFF
                    val b = buffer.get(pos + 2).toInt() and 0xFF
                    val a = buffer.get(pos + 3).toInt() and 0xFF
                    pixels[idx] = (a shl 24) or (r shl 16) or (g shl 8) or b
                    pos += pixelStride
                    idx++
                }
            }
            return Frame(w, h, pixels, System.currentTimeMillis())
        } finally {
            image.close()
        }
    }

    fun stop() {
        virtualDisplay?.release()
        virtualDisplay = null
        imageReader?.close()
        imageReader = null
    }
}
