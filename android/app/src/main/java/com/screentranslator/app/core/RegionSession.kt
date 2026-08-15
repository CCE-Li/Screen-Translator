package com.screentranslator.app.core

import com.screentranslator.app.ocr.OcrProvider

sealed class UpdateResult {
    object NoChange : UpdateResult()
    data class Redraw(val overlays: List<OverlayText>) : UpdateResult()
}

/** Interval performance counters for one session. */
class SessionStats {
    var ocrRuns = 0L
    var framesSeen = 0L
    var ocrMs = 0L
    var ocrCacheHits = 0L
    var tcacheHits = 0L
    var tcacheMisses = 0L
    var pending = 0
}

/**
 * Per-region live pipeline session (mirror of the Rust core `RegionSession`).
 *
 * Owns the OCR provider, translation cache and OCR-result cache, and decides
 * when to run OCR (frame diff + cooldown + retrigger), what to re-translate
 * (cache lookup + in-flight dedup) and what the overlay should draw.
 *
 * Must be owned by a single thread. Translation I/O happens outside; results
 * are applied back via `acceptTranslations`.
 */
class RegionSession(
    val id: String,
    private val ocr: OcrProvider,
    private val ocrCooldownMs: Long,
    private val ocrRetriggerMs: Long,
    private val diffThreshold: Float,
    val sourceLang: String?,
    val targetLang: String,
    cacheCapacity: Int = 2048,
) {
    private val translationCache = TranslationCache(cacheCapacity)
    private val textRegionCache = LruCache<String, TextRegion>(512)
    private var lastSignature: IntArray? = null
    private var lastRegions: List<TextRegion> = emptyList()
    private var lastOcrTime = 0L
    private var lastFailed = 0L
    private val pending = HashSet<String>()
    private val queued = ArrayDeque<TranslationTask>()
    val stats = SessionStats()

    fun ocrName(): String = ocr.name()

    fun processFrame(frame: Frame, nowMs: Long): UpdateResult {
        stats.framesSeen++

        val sig = FrameDiff.signature(frame)
        val changed = lastSignature?.let { FrameDiff.changedFraction(it, sig) } ?: 1f
        lastSignature = sig

        val cooldownOk = nowMs - lastOcrTime >= ocrCooldownMs
        val force = nowMs - lastOcrTime >= ocrRetriggerMs
        val changedEnough = changed > diffThreshold
        if (!(cooldownOk && (changedEnough || force))) return UpdateResult.NoChange

        lastOcrTime = nowMs
        val t0 = System.nanoTime()
        val regions = try {
            ocr.recognize(frame)
        } catch (e: Exception) {
            android.util.Log.w("RegionSession", "[$id] OCR failed: ${e.message}")
            emptyList()
        }
        stats.ocrMs = (System.nanoTime() - t0) / 1_000_000
        stats.ocrRuns++

        val texts = regions.map { TranslationCache.normalize(it.text) }.filter { it.isNotEmpty() }
        val prevTexts = lastRegions.map { TranslationCache.normalize(it.text) }.filter { it.isNotEmpty() }
        val textsChanged = texts != prevTexts
        lastRegions = regions

        val newTexts = ArrayList<String>()
        for (r in regions) {
            val key = TranslationCache.normalize(r.text)
            if (key.isEmpty()) continue
            if (translationCache.get(sourceLang ?: "", targetLang, key) != null) {
                stats.tcacheHits++
                continue
            }
            stats.tcacheMisses++
            if (key in pending) continue
            pending.add(key)
            newTexts.add(r.text)
        }
        if (newTexts.isNotEmpty()) {
            queued.addLast(TranslationTask(id, sourceLang, targetLang, newTexts))
        }
        stats.pending = pending.size

        for (r in regions) {
            val key = TranslationCache.normalize(r.text)
            if (key.isNotEmpty()) textRegionCache.put(key, r)
        }

        return if (textsChanged) UpdateResult.Redraw(buildOverlays()) else UpdateResult.NoChange
    }

    /** Take pending translation work for the caller to run. */
    fun drainRequests(): List<TranslationTask> {
        val all = queued.toList()
        queued.clear()
        return all
    }

    fun acceptTranslations(task: TranslationTask, results: List<Translation>) {
        for ((text, t) in task.texts.zip(results)) {
            val key = TranslationCache.normalize(text)
            translationCache.put(task.sourceLang ?: "", task.targetLang, key, t.translatedText)
            pending.remove(key)
        }
        stats.pending = pending.size
    }

    /** Called when a request permanently failed; releases pending markers (rate-limited). */
    fun noteFailure(task: TranslationTask) {
        val now = System.currentTimeMillis()
        if (now - lastFailed >= 5000) {
            for (t in task.texts) pending.remove(TranslationCache.normalize(t))
            lastFailed = now
            stats.pending = pending.size
        }
    }

    fun refreshOverlays(): List<OverlayText> = buildOverlays()

    fun translationCacheSize(): Int = translationCache.size()

    fun translationCacheHitRate(): Float = translationCache.hitRate()

    fun takeStatsDelta(): SessionStats {
        val d = stats
        // The caller reads and then resets through resetStats().
        return d
    }

    fun resetStats() {
        stats.ocrRuns = 0
        stats.framesSeen = 0
        stats.ocrMs = 0
        stats.ocrCacheHits = 0
        stats.tcacheHits = 0
        stats.tcacheMisses = 0
    }

    private fun buildOverlays(): List<OverlayText> {
        val out = ArrayList<OverlayText>()
        for (region in lastRegions) {
            val key = TranslationCache.normalize(region.text)
            if (key.isEmpty()) continue
            val translated = translationCache.get(sourceLang ?: "", targetLang, key) ?: continue
            val srcRegion = textRegionCache.get(key) ?: region
            out.add(layoutOverlay(srcRegion, translated))
        }
        return out
    }

    private fun layoutOverlay(region: TextRegion, translated: String): OverlayText {
        val box = region.boundingBox
        val base = if (region.fontSize > 0f) region.fontSize else (box.height * 0.85f).coerceIn(8f, 120f)
        var fontSize = base.coerceIn(6f, 48f)
        var needed = estimateWidth(translated, fontSize)
        var guard = 0
        while (needed > box.width && fontSize > 6f && guard < 24) {
            fontSize *= 0.9f
            needed = estimateWidth(translated, fontSize)
            guard++
        }
        return OverlayText(translated, box, fontSize, TextAlign.Center)
    }

    private fun estimateWidth(text: String, fontSize: Float): Float {
        var w = 0f
        for (c in text) {
            w += if (isCjk(c)) fontSize else if (c == ' ') fontSize * 0.33f else fontSize * 0.55f
        }
        return w
    }

    private fun isCjk(c: Char): Boolean {
        val code = c.code
        return code in 0x2E80..0x2EFF || code in 0x3000..0x303F || code in 0x3040..0x30FF ||
            code in 0x3400..0x4DBF || code in 0x4E00..0x9FFF || code in 0xF900..0xFAFF ||
            code in 0xAC00..0xD7AF
    }
}
