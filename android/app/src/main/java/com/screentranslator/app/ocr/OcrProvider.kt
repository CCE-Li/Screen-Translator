package com.screentranslator.app.ocr

import com.screentranslator.app.core.Frame
import com.screentranslator.app.core.TextRegion

/** OCR provider abstraction. The pipeline only talks to this interface. */
interface OcrProvider {
    fun name(): String

    fun availableLanguages(): List<String>

    /** Recognize all text in the frame; returns region-local bounding boxes. */
    fun recognize(frame: Frame): List<TextRegion>
}
