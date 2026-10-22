//! A small least-recently-used cache.

use std::collections::{HashMap, VecDeque};

/// Small least-recently-used cache: album details (a few KB each) and encoded cover art.
#[derive(Debug)]
pub(super) struct Lru<K, V> {
    capacity: usize,
    map: HashMap<K, V>,
    /// Most recently used at the back.
    order: VecDeque<K>,
}

impl<K: Copy + Eq + std::hash::Hash, V: Clone> Lru<K, V> {
    /// Allocates nothing until the first insert, so an unused cache (e.g. art while it's
    /// disabled) costs nothing.
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Looks up without counting as a use (for rendering).
    pub(super) fn peek(&self, key: K) -> Option<&V> {
        self.map.get(&key)
    }

    /// Looks up and marks as recently used.
    pub(super) fn get(&mut self, key: K) -> Option<V> {
        let value = self.map.get(&key)?.clone();
        self.touch(key);
        Some(value)
    }

    pub(super) fn insert(&mut self, key: K, value: V) {
        if self.map.insert(key, value).is_some() {
            self.touch(key);
            return;
        }
        self.order.push_back(key);
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.map.remove(&oldest);
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map.iter()
    }

    pub(super) fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        self.map.retain(|k, _| keep(k));
        self.order.retain(|k| keep(k));
    }

    pub(super) fn remove(&mut self, key: K) {
        if self.map.remove(&key).is_some() {
            self.order.retain(|&k| k != key);
        }
    }

    pub(super) fn touch(&mut self, key: K) {
        if let Some(pos) = self.order.iter().position(|&k| k == key) {
            self.order.remove(pos);
            self.order.push_back(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let mut cache: Lru<u32, &str> = Lru::new(2);
        cache.insert(1, "one");
        cache.insert(2, "two");
        cache.get(1); // 1 is now more recent than 2
        cache.insert(3, "three");
        assert!(cache.peek(1).is_some());
        assert!(cache.peek(2).is_none());
        assert!(cache.peek(3).is_some());
        assert_eq!(cache.map.len(), 2);
        assert_eq!(cache.order.len(), 2);
    }
}
