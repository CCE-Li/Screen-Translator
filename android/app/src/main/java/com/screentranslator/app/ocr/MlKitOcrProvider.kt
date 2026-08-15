package com.screentranslator.app.ocr

import android.graphics.Bitmap
import android.util.Log
import com.google.android.gms.tasks.Tasks
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognizer
import com.screentranslator.app.core.Frame
import com.screentranslator.app.core.RectF
import com.screentranslator.app.core.TextRegion
import java.util.concurrent.TimeUnit

/**
 * Google ML Kit on-device text recognition (classic, via Play services).
 * `recognize` blocks and must be called off the main thread.
 *
 * The first calls may block up to [waitTimeoutSecs] while the OCR engine/module
 * is downloaded or initialized; afterwards recognition is fast.
 */
class MlKitOcrProvider(
    private val recognizer: TextRecognizer,
    private val languageTag: String,
    private val waitTimeoutSecs: Long = 6,
) : OcrProvider {

    override fun name(): String = "mlkit"

    override fun availableLanguages(): List<String> = listOf(languageTag)

    override fun recognize(frame: Frame): List<TextRegion> {
        val bitmap = Bitmap.createBitmap(frame.width, frame.height, Bitmap.Config.ARGB_8888)
        bitmap.setPixels(frame.pixels, 0, frame.width, 0, 0, frame.width, frame.height)
        val image = InputImage.fromBitmap(bitmap, 0)
        val result = try {
            Tasks.await(recognizer.process(image), waitTimeoutSecs, TimeUnit.SECONDS)
        } finally {
            bitmap.recycle()
        }
        val out = ArrayList<TextRegion>()
        for (block in result.textBlocks) {
            for (line in block.lines) {
                val b = line.boundingBox
                if (b == null || b.width() <= 0) continue
                val text = line.text.trim()
                if (text.isEmpty()) continue
                val box = RectF(b.left.toFloat(), b.top.toFloat(), b.width().toFloat(), b.height().toFloat())
                out.add(
                    TextRegion(
                        text = text,
                        confidence = 1f, // ML Kit does not expose per-line confidence here
                        boundingBox = box,
                        language = languageTag,
                        fontSize = box.height.coerceAtLeast(8f),
                    )
                )
            }
        }
        if (out.isNotEmpty()) {
            val sample = out.take(5).joinToString(" | ") { it.text }
            Log.d("MlKitOcr", "recognized ${out.size} line(s): $sample")
        }
        return out
    }
}
