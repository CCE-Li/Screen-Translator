package com.screentranslator.app.core

/**
 * Thread-safe LRU cache backed by an access-ordered LinkedHashMap. Optional TTL
 * is intentionally omitted to keep it simple; callers prune when appropriate.
 */
class LruCache<K, V>(private val capacity: Int) {
    private val map = object : LinkedHashMap<K, V>(capacity, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<K, V>?): Boolean = size > capacity
    }
    private var hits = 0L
    private var misses = 0L

    @Synchronized
    fun get(key: K): V? {
        val v = map[key]
        if (v != null) hits++ else misses++
        return v
    }

    @Synchronized
    fun put(key: K, value: V) {
        map[key] = value
    }

    @Synchronized
    fun remove(key: K): V? = map.remove(key)

    @Synchronized
    fun clear() = map.clear()

    @Synchronized
    fun size(): Int = map.size

    @Synchronized
    fun hitRate(): Float {
        val total = hits + misses
        return if (total == 0L) 0f else hits.toFloat() / total
    }
}
