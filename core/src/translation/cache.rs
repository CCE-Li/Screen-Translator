//! Translation result cache. Keyed by (source, target, normalized text).
//! In-memory LRU with optional TTL, so repeated text never re-hits the API.

use std::time::Duration;

use crate::cache::LruCache;
use crate::translation::normalize_text;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    src: String,
    tgt: String,
    text: String,
}

pub struct TranslationCache {
    inner: LruCache<CacheKey, String>,
}

impl TranslationCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: LruCache::new(capacity),
        }
    }

    pub fn with_ttl(capacity: usize, ttl: Duration) -> Self {
        Self {
            inner: LruCache::with_ttl(capacity, ttl),
        }
    }

    /// Look up a translation. `src` may be empty for auto-detect.
    pub fn get(&mut self, src: &str, tgt: &str, text: &str) -> Option<String> {
        let key = CacheKey {
            src: src.to_string(),
            tgt: tgt.to_string(),
            text: normalize_text(text),
        };
        self.inner.get(&key).cloned()
    }

    pub fn insert(&mut self, src: &str, tgt: &str, text: &str, translation: String) {
        let key = CacheKey {
            src: src.to_string(),
            tgt: tgt.to_string(),
            text: normalize_text(text),
        };
        self.inner.insert(key, translation);
    }

    pub fn hit_rate(&self) -> f32 {
        self.inner.hit_rate()
    }

    pub fn stats(&self) -> (u64, u64) {
        self.inner.stats()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_normalizes_whitespace() {
        let mut c = TranslationCache::new(10);
        assert!(c.get("ja", "zh", "Iron  Sword").is_none());
        c.insert("ja", "zh", "Iron Sword", "铁剑".into());
        assert_eq!(c.get("ja", "zh", "  Iron   Sword "), Some("铁剑".into()));
    }

    #[test]
    fn cache_miss_on_different_language_pair() {
        let mut c = TranslationCache::new(10);
        c.insert("ja", "zh", "Iron Sword", "铁剑".into());
        assert!(c.get("en", "zh", "Iron Sword").is_none());
    }

    #[test]
    fn stats_track_hits() {
        let mut c = TranslationCache::new(10);
        c.insert("en", "zh", "hi", "你好".into());
        let _ = c.get("en", "zh", "hi");
        let _ = c.get("en", "zh", "missing");
        assert_eq!(c.stats(), (1, 1));
        assert!((c.hit_rate() - 0.5).abs() < 1e-6);
    }
}
