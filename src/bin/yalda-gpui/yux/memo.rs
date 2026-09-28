//! `KeyedMemo` — a one-slot memo for a derived list (a filter's match list, a
//! palette's ranking) keyed on everything it is a pure function of: typically
//! `(query text, source generation)`. Render paths and key handlers both ask
//! for the value; only a key change recomputes, so a frame that changes
//! neither the query nor the source reuses the previous result instead of
//! re-ranking (A9). Interior-mutable so `&self` render/read paths can fill it.
//!
//! Each recompute is counted under `label` via `record_render`, so a test can
//! assert "a render without change did no re-rank" with `perf_render_count`.

use std::cell::RefCell;
use std::rc::Rc;

use super::record_render;

pub(crate) struct KeyedMemo<K, V> {
    label: &'static str,
    slot: RefCell<Option<(K, Rc<V>)>>,
}

impl<K: PartialEq, V> KeyedMemo<K, V> {
    pub(crate) fn new(label: &'static str) -> Self {
        KeyedMemo {
            label,
            slot: RefCell::new(None),
        }
    }

    /// The memoized value for `key`, computing (and counting) it only when the
    /// key differs from the cached one.
    pub(crate) fn get_or_compute(&self, key: K, compute: impl FnOnce() -> V) -> Rc<V> {
        if let Some((k, v)) = &*self.slot.borrow()
            && *k == key
        {
            return v.clone();
        }
        record_render(self.label);
        let v = Rc::new(compute());
        *self.slot.borrow_mut() = Some((key, v.clone()));
        v
    }

    /// Drop the cached value (the next read recomputes).
    #[allow(dead_code)]
    pub(crate) fn invalidate(&self) {
        *self.slot.borrow_mut() = None;
    }
}

/// A cheap order-sensitive fingerprint of a sequence of strings — the "source
/// generation" for derived lists whose source has no explicit version counter
/// (the ranking is a pure function of exactly these strings).
pub(crate) fn fingerprint_strs<'a>(items: impl IntoIterator<Item = &'a str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut n = 0usize;
    for s in items {
        s.hash(&mut h);
        n += 1;
    }
    n.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_memo_recomputes_only_on_key_change() {
        crate::perf_reset("memo_test");
        let m: KeyedMemo<(String, u64), Vec<usize>> = KeyedMemo::new("memo_test");
        let a = m.get_or_compute(("q".into(), 1), || vec![1]);
        let b = m.get_or_compute(("q".into(), 1), || vec![2]);
        assert_eq!(*b, vec![1], "same key reuses the cached value");
        assert!(Rc::ptr_eq(&a, &b));
        assert_eq!(crate::perf_render_count("memo_test"), 1);
        let c = m.get_or_compute(("qq".into(), 1), || vec![3]);
        assert_eq!(*c, vec![3]);
        let d = m.get_or_compute(("qq".into(), 2), || vec![4]);
        assert_eq!(*d, vec![4]);
        assert_eq!(crate::perf_render_count("memo_test"), 3);
    }
}
