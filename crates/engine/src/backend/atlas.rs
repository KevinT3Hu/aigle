//! Slot bookkeeping of the GPU source tile atlas (doc 03 §4.2).
//!
//! The atlas is one device buffer of fixed-size slots, each holding one
//! [`ATLAS_TILE`](aigle_kernels::tiles::ATLAS_TILE)² tile of a source pyramid level. A batch
//! of C1 tiles names the source tiles it reads; [`AtlasState::assign`] maps them to slots,
//! reusing resident tiles and evicting the least recently used tiles that the batch does
//! not need. The GPU executes submissions in order, so a slot can be overwritten as soon as
//! the submissions that read it have been *recorded*: the caller holds the atlas lock from
//! assignment through `queue.submit`.

use std::collections::HashMap;

use crate::cache::CacheKey;

/// One source tile: (A-tier key of the source, level, tile x, tile y).
pub type TileId = (CacheKey, u32, u32, u32);

#[derive(Default)]
pub struct AtlasState {
    /// Per slot: resident tile and last use.
    slots: Vec<Option<(TileId, u64)>>,
    map: HashMap<TileId, u32>,
    tick: u64,
}

impl AtlasState {
    pub fn new(slots: u32) -> AtlasState {
        AtlasState {
            slots: vec![None; slots as usize],
            ..Default::default()
        }
    }

    pub fn len(&self) -> u32 {
        self.slots.len() as u32
    }

    /// Add empty slots at the end (the buffer grew).
    pub fn grow(&mut self, slots: u32) {
        self.slots.resize(slots as usize, None);
    }

    /// Number of `want` tiles that are not resident.
    pub fn misses(&self, want: &[TileId]) -> usize {
        want.iter().filter(|t| !self.map.contains_key(t)).count()
    }

    fn free_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_none()).count()
    }

    /// Whether `misses` new tiles fit without evicting.
    pub fn fits_without_eviction(&self, misses: usize) -> bool {
        misses <= self.free_slots()
    }

    /// Slots for `want` (distinct, `want.len() ≤ len()`), and the indices of `want` whose
    /// tiles must be uploaded into their slot.
    pub fn assign(&mut self, want: &[TileId]) -> (Vec<u32>, Vec<usize>) {
        assert!(
            want.len() <= self.slots.len(),
            "batch larger than the atlas"
        );
        self.tick += 1;
        let tick = self.tick;
        let mut out = vec![u32::MAX; want.len()];
        let mut missing = Vec::new();
        for (i, t) in want.iter().enumerate() {
            match self.map.get(t) {
                Some(&s) => {
                    self.slots[s as usize] = Some((*t, tick));
                    out[i] = s;
                }
                None => missing.push(i),
            }
        }
        if missing.is_empty() {
            return (out, missing);
        }
        // Free slots first, then least recently used; never a slot this batch uses.
        let mut cand: Vec<(u64, u32)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(s, e)| match e {
                None => Some((0, s as u32)),
                Some((_, t)) if *t < tick => Some((*t, s as u32)),
                Some(_) => None,
            })
            .collect();
        cand.sort_unstable();
        for (&i, &(_, s)) in missing.iter().zip(&cand) {
            if let Some((old, _)) = self.slots[s as usize].take() {
                self.map.remove(&old);
            }
            self.slots[s as usize] = Some((want[i], tick));
            self.map.insert(want[i], s);
            out[i] = s;
        }
        (out, missing)
    }

    #[cfg(test)]
    pub fn resident(&self, t: &TileId) -> bool {
        self.map.contains_key(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::KeyBuilder;

    fn id(i: u32) -> TileId {
        (KeyBuilder::new("s").finish(), 0, i, 0)
    }

    #[test]
    fn reuses_and_evicts_lru_outside_the_batch() {
        let mut a = AtlasState::new(4);
        let (s1, up1) = a.assign(&[id(0), id(1), id(2)]);
        assert_eq!(up1, [0, 1, 2]);
        let (s2, up2) = a.assign(&[id(1), id(3)]);
        assert_eq!(up2, [1]);
        assert_eq!(s2[0], s1[1], "resident tile reused");
        // Full: the next miss evicts the LRU tile (0 or 2, used before id(1)/id(3)).
        let (s3, up3) = a.assign(&[id(4), id(1)]);
        assert_eq!(up3, [0]);
        assert!(s3[0] == s1[0] || s3[0] == s1[2]);
        assert!(a.resident(&id(1)) && a.resident(&id(3)) && a.resident(&id(4)));
        let mut slots = s3.clone();
        slots.sort();
        slots.dedup();
        assert_eq!(slots.len(), 2);
        a.grow(6);
        assert!(a.fits_without_eviction(2));
    }
}
