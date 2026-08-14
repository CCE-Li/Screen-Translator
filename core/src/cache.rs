//! A small, dependency-free LRU cache with optional TTL. Used for OCR and
//! translation result caching.

use std::collections::HashMap;
use std::time::{Duration, Instant};

struct Entry<V> {
    value: V,
    last_used: Instant,
}

/// Least-recently-used cache with optional time-to-live.
///
/// `get` updates recency, `insert` evicts the least recently used entry when
/// over capacity. Entries whose TTL expired are dropped lazily.
pub struct LruCache<K, V> {
    capacity: usize,
    ttl: Option<Duration>,
    map: HashMap<K, Entry<V>>,
    // LRU order: front = most recently used. Only contains live keys.
    order: std::collections::VecDeque<K>,
    hits: u64,
    misses: u64,
}

impl<K, V> LruCache<K, V>
where
    K: Eq + std::hash::Hash + Clone,
{
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            ttl: None,
            map: HashMap::with_capacity(capacity),
            order: std::collections::VecDeque::new(),
            hits: 0,
            misses: 0,
        }
    }

    pub fn with_ttl(capacity: usize, ttl: Duration) -> Self {
        let mut s = Self::new(capacity);
        s.ttl = Some(ttl);
        s
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.is_expired(key) {
            self.remove(key);
            self.misses += 1;
            return None;
        }
        let Some(entry) = self.map.get_mut(key) else {
            self.misses += 1;
            return None;
        };
        entry.last_used = Instant::now();
        let _ = entry;
        self.touch(key);
        self.hits += 1;
        self.map.get(key).map(|e| &e.value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }
        if let Some(entry) = self.map.get_mut(&key) {
            entry.value = value;
            entry.last_used = Instant::now();
            self.touch(&key);
            return;
        }
        if self.map.len() >= self.capacity {
            self.evict_lru();
        }
        self.map.insert(
            key.clone(),
            Entry {
                value,
                last_used: Instant::now(),
            },
        );
        self.order.push_front(key);
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.map.remove(key).map(|e| e.value)
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Cache hit rate as a fraction 0.0..=1.0.
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f32 / total as f32
        }
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    fn is_expired(&self, key: &K) -> bool {
        let Some(ttl) = self.ttl else {
            return false;
        };
        matches!(self.map.get(key), Some(e) if e.last_used.elapsed() > ttl)
    }

    fn touch(&mut self, key: &K) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_front(key.clone());
    }

    fn evict_lru(&mut self) {
        while let Some(key) = self.order.pop_back() {
            if self.map.remove(&key).is_some() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut c: LruCache<&str, i32> = LruCache::new(3);
        c.insert("a", 1);
        c.insert("b", 2);
        c.insert("c", 3);
        assert_eq!(c.get(&"a"), Some(&1)); // touch a
        c.insert("d", 4); // evict b (LRU)
        assert!(c.get(&"b").is_none());
        assert_eq!(c.get(&"a"), Some(&1));
        assert_eq!(c.get(&"c"), Some(&3));
        assert_eq!(c.get(&"d"), Some(&4));
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn ttl_expires_entries() {
        let mut c: LruCache<&str, i32> = LruCache::with_ttl(10, Duration::from_millis(1));
        c.insert("a", 1);
        std::thread::sleep(Duration::from_millis(5));
        assert!(c.get(&"a").is_none());
    }

    #[test]
    fn insert_existing_updates() {
        let mut c: LruCache<&str, i32> = LruCache::new(2);
        c.insert("a", 1);
        c.insert("a", 2);
        assert_eq!(c.len(), 1);
        assert_eq!(c.get(&"a"), Some(&2));
    }

    #[test]
    fn capacity_zero_stores_nothing() {
        let mut c: LruCache<&str, i32> = LruCache::new(0);
        c.insert("a", 1);
        assert_eq!(c.len(), 0);
    }
}
