package com.screentranslator.app.core

/** Translation result cache keyed by (source, target, normalized text). */
class TranslationCache(capacity: Int) {
    private data class Key(val src: String, val tgt: String, val text: String)

    private val lru = LruCache<Key, String>(capacity)

    @Synchronized
    fun get(src: String, tgt: String, text: String): String? =
        lru.get(Key(src, tgt, normalize(text)))

    @Synchronized
    fun put(src: String, tgt: String, text: String, translated: String) {
        lru.put(Key(src, tgt, normalize(text)), translated)
    }

    fun hitRate(): Float = lru.hitRate()

    fun size(): Int = lru.size()

    companion object {
        /** Trim and collapse runs of whitespace for cache keys. */
        fun normalize(text: String): String =
            text.trim().split(Regex("\\s+")).joinToString(" ")
    }
}
