package com.screentranslator.app.core

/** Normalized rectangle in pixels (screen or region-local depending on context). */
data class RectF(
    val x: Float,
    val y: Float,
    val width: Float,
    val height: Float,
) {
    fun contains(px: Float, py: Float): Boolean =
        px >= x && px < x + width && py >= y && py < y + height

    fun toAndroidRect(): android.graphics.Rect =
        android.graphics.Rect(x.toInt(), y.toInt(), (x + width).toInt(), (y + height).toInt())
}

/** One block of recognized text with its bounding box (region-local coords). */
data class TextRegion(
    val text: String,
    val confidence: Float,
    val boundingBox: RectF,
    val language: String?,
    val fontSize: Float,
)

/** A captured frame. Pixels are ARGB8888 (alpha=0xFF), top-left origin. */
class Frame(
    val width: Int,
    val height: Int,
    val pixels: IntArray,
    val timestamp: Long,
) {
    val stride: Int get() = width
}

data class Translation(
    val translatedText: String,
    val detectedSourceLang: String?,
    val targetLang: String,
)

enum class TextAlign { Left, Center, Right }

/** A finalized overlay instruction (screen coordinates). */
data class OverlayText(
    val text: String,
    val box: RectF,
    val fontSize: Float,
    val align: TextAlign,
)

/** A batch of source texts that still need translation. */
data class TranslationTask(
    val regionId: String,
    val sourceLang: String?,
    val targetLang: String,
    val texts: List<String>,
)
