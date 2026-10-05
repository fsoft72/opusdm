//! The order in which the desktop background entries come up. Pure logic, no
//! GTK: the controller in `background.rs` asks it which entry is next.

/// Walks the indexes `0..len` in list order, or in a shuffled order that is
/// drawn again every time the walk wraps around.
#[derive(Debug, Clone)]
pub struct Sequence {
    len: usize,
    shuffle: bool,
    order: Vec<usize>,
    pos: usize,
    last: Option<usize>,
    rng: u64,
}

impl Sequence {
    /// A sequence over `len` entries. `seed` feeds the shuffle only.
    pub fn new(len: usize, shuffle: bool, seed: u64) -> Self {
        let mut seq = Sequence { len, shuffle, order: Vec::new(), pos: 0, last: None, rng: seed | 1 };
        seq._refill();
        seq
    }

    /// The next index for which `usable` is true, or `None` when no entry is
    /// usable. Each call advances the walk, so an unusable entry is skipped,
    /// never retried within the same pass.
    pub fn next(&mut self, usable: impl Fn(usize) -> bool) -> Option<usize> {
        // One full pass plus the refill that can happen in the middle of it.
        for _ in 0..self.len * 2 {
            if self.pos >= self.order.len() {
                self._refill();
            }
            let idx = *self.order.get(self.pos)?;
            self.pos += 1;
            if usable(idx) {
                self.last = Some(idx);
                return Some(idx);
            }
        }
        None
    }

    /// Start a new pass. A shuffled pass never opens with the entry the
    /// previous pass ended on (when there is another one to open with).
    fn _refill(&mut self) {
        self.order = (0..self.len).collect();
        self.pos = 0;
        if !self.shuffle {
            return;
        }
        for i in (1..self.order.len()).rev() {
            let j = (self._random() % (i as u64 + 1)) as usize;
            self.order.swap(i, j);
        }
        if self.len > 1 && self.order.first().copied() == self.last {
            self.order.swap(0, 1);
        }
    }

    /// xorshift64: plenty for picking a wallpaper order.
    fn _random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(seq: &mut Sequence, n: usize) -> Vec<usize> {
        (0..n).filter_map(|_| seq.next(|_| true)).collect()
    }

    #[test]
    fn list_order_wraps_around() {
        let mut seq = Sequence::new(3, false, 1);
        assert_eq!(take(&mut seq, 7), vec![0, 1, 2, 0, 1, 2, 0]);
    }

    #[test]
    fn a_single_entry_comes_up_again_and_again() {
        let mut seq = Sequence::new(1, true, 9);
        assert_eq!(take(&mut seq, 3), vec![0, 0, 0]);
    }

    #[test]
    fn nothing_to_walk_yields_none() {
        let mut seq = Sequence::new(0, false, 1);
        assert_eq!(seq.next(|_| true), None);
    }

    #[test]
    fn unusable_entries_are_skipped() {
        let mut seq = Sequence::new(4, false, 1);
        let got: Vec<usize> = (0..4).filter_map(|_| seq.next(|i| i % 2 == 1)).collect();
        assert_eq!(got, vec![1, 3, 1, 3]);
    }

    #[test]
    fn all_unusable_yields_none() {
        let mut seq = Sequence::new(3, false, 1);
        assert_eq!(seq.next(|_| false), None);
        assert_eq!(seq.next(|_| false), None);
    }

    #[test]
    fn a_shuffled_pass_visits_every_entry_once() {
        for seed in 1..40 {
            let mut seq = Sequence::new(6, true, seed);
            let mut pass = take(&mut seq, 6);
            pass.sort();
            assert_eq!(pass, vec![0, 1, 2, 3, 4, 5], "seed {seed}");
        }
    }

    #[test]
    fn a_shuffled_pass_never_opens_with_the_entry_that_just_played() {
        for seed in 1..60 {
            let mut seq = Sequence::new(3, true, seed);
            let got = take(&mut seq, 30);
            assert!(got.windows(2).all(|w| w[0] != w[1]), "seed {seed}: {got:?}");
        }
    }

    #[test]
    fn shuffle_changes_the_order_for_some_seed() {
        let ordered: Vec<usize> = (0..8).collect();
        let differs = (1..20).any(|seed| take(&mut Sequence::new(8, true, seed), 8) != ordered);
        assert!(differs);
    }
}
