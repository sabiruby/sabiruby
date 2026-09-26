//! A sorted sequence that is cheap at both ends: what mruby-task's queues and their indexes are
//! kept in (`vm::TaskQueue`, `TaskState::sleepers` / `waiters`).
//!
//! The scheduler's keys mostly arrive in order — a task joins the back of its queue (the key ends
//! in a sequence number that only grows), and the one that leaves is the one in front — so an
//! insert at the back and a removal from the front are O(1) here. Anything else finds its place
//! by binary search, O(log n), and then shifts the shorter side of the `VecDeque`. A `BTreeMap`
//! does every one of these in O(log n) but pays for its tree even for two tasks; this measured
//! 10–30% faster on a scheduler with ten tasks and a queue ping-pong, and as fast with a thousand
//! sleepers (`docs/worklog/2026-09-27-release-0.7-bench.md`).

use alloc::collections::VecDeque;

/// Sorted by `K`; every key is in it at most once (the callers' keys end in a sequence number).
pub struct SortedDeque<K, V> {
    items: VecDeque<(K, V)>,
}

impl<K, V> Default for SortedDeque<K, V> {
    fn default() -> Self { SortedDeque { items: VecDeque::new() } }
}

impl<K: Ord + Copy, V: Copy> SortedDeque<K, V> {
    pub fn len(&self) -> usize { self.items.len() }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }

    /// Puts `v` at key `k` (which must not be there already).
    pub fn insert(&mut self, k: K, v: V) {
        match self.items.back() {
            Some((last, _)) if *last > k => {
                let i = self.items.partition_point(|(x, _)| *x < k);
                self.items.insert(i, (k, v));
            }
            _ => self.items.push_back((k, v)),
        }
    }

    /// Takes key `k` out, answering what it held.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        if let Some((first, _)) = self.items.front() { if first == k { return self.items.pop_front().map(|(_, v)| v); } }
        if let Some((last, _)) = self.items.back() { if last == k { return self.items.pop_back().map(|(_, v)| v); } }
        let i = self.items.binary_search_by(|(x, _)| x.cmp(k)).ok()?;
        self.items.remove(i).map(|(_, v)| v)
    }

    /// The smallest key and its value.
    pub fn first_key_value(&self) -> Option<(&K, &V)> { self.items.front().map(|(k, v)| (k, v)) }

    /// The entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> + '_ { self.items.iter().map(|(k, v)| (k, v)) }

    /// The entries from the first key not below `lo` on, in key order.
    pub fn iter_from(&self, lo: &K) -> impl Iterator<Item = (&K, &V)> + '_ {
        let i = self.items.partition_point(|(x, _)| x < lo);
        self.items.range(i..).map(|(k, v)| (k, v))
    }

    /// Keeps the entries `keep` answers true for.
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) { self.items.retain(|(k, v)| keep(k, v)); }
}
