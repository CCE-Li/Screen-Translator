package com.screentranslator.app.core

/**
 * Cheap frame-change detection: sample a coarse grid of average-luminance
 * cells. Two frames are compared by the fraction of cells that changed, so the
 * pipeline can skip OCR on static content.
 */
object FrameDiff {
    const val SAMPLE_GRID = 24
    private const val CELL_CHANGE_DELTA = 24

    fun signature(frame: Frame): IntArray {
        val w = frame.width
        val h = frame.height
        val out = IntArray(SAMPLE_GRID * SAMPLE_GRID)
        if (w <= 0 || h <= 0) return out
        val cw = w.toFloat() / SAMPLE_GRID
        val ch = h.toFloat() / SAMPLE_GRID
        var idx = 0
        for (gy in 0 until SAMPLE_GRID) {
            val sy = (gy * ch).toInt()
            val ey = minOf(((gy + 1) * ch).toInt(), h)
            for (gx in 0 until SAMPLE_GRID) {
                val sx = (gx * cw).toInt()
                val ex = minOf(((gx + 1) * cw).toInt(), w)
                if (ex <= sx || ey <= sy) {
                    out[idx++] = 0
                    continue
                }
                var r = 0L
                var g = 0L
                var b = 0L
                var count = 0L
                for (y in sy until ey) {
                    val row = y * w
                    for (x in sx until ex) {
                        val p = frame.pixels[row + x]
                        r += (p shr 16) and 0xFF
                        g += (p shr 8) and 0xFF
                        b += p and 0xFF
                        count++
                    }
                }
                out[idx++] = ((30 * r + 59 * g + 11 * b) / (100 * count)).toInt()
            }
        }
        return out
    }

    /** Fraction (0..1) of sampled cells that changed noticeably. */
    fun changedFraction(a: IntArray, b: IntArray): Float {
        if (a.size != b.size) return 1f
        var changed = 0
        for (i in a.indices) {
            if (kotlin.math.abs(a[i] - b[i]) >= CELL_CHANGE_DELTA) changed++
        }
        return changed.toFloat() / a.size
    }
}
