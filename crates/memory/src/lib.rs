#![no_std]

use hpos_boot_protocol::MemoryEntry;

pub const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidMap,
    Unaligned,
    NotAllocated,
}

/// One free bitmap and one ownership bitmap. No heap or architecture-specific code.
/// The caller must reserve its kernel, firmware and device ranges before allocating.
pub struct FrameAllocator<const WORDS: usize> {
    free: [u64; WORDS],
    owned: [u64; WORDS],
    count: usize,
    cursor: usize,
}

impl<const WORDS: usize> FrameAllocator<WORDS> {
    pub const fn empty() -> Self {
        Self {
            free: [0; WORDS],
            owned: [0; WORDS],
            count: 0,
            cursor: 0,
        }
    }

    pub fn initialize(&mut self, map: &[MemoryEntry], reserved: &[Region]) -> Result<(), Error> {
        self.free.fill(0);
        self.owned.fill(0);
        self.count = 0;
        self.cursor = 0;
        // Validate the whole map before exposing any free frames.
        if map.is_empty()
            || map.iter().any(|e| e.base.checked_add(e.length).is_none())
            || reserved.iter().any(|r| r.start > r.end)
        {
            return Err(Error::InvalidMap);
        }
        for entry in map {
            if entry.kind == 1 && entry.attributes & 1 != 0 {
                self.mark(
                    Region {
                        start: entry.base,
                        end: entry.base + entry.length,
                    },
                    true,
                );
            }
        }
        // Reserved E820 entries take precedence even for unsorted, overlapping maps.
        for entry in map {
            if entry.kind != 1 && entry.attributes & 1 != 0 {
                self.mark(
                    Region {
                        start: entry.base,
                        end: entry.base + entry.length,
                    },
                    false,
                );
            }
        }
        for &region in reserved {
            self.mark(region, false);
        }
        Ok(())
    }

    fn mark(&mut self, region: Region, available: bool) {
        let limit = (WORDS as u64) * 64 * PAGE_SIZE;
        let start = region.start.min(limit);
        let end = region.end.min(limit);
        if start >= end {
            return;
        }
        // Free only complete pages; reserve any page touched by a reserved byte.
        let first = if available {
            start.div_ceil(PAGE_SIZE)
        } else {
            start / PAGE_SIZE
        } as usize;
        let last = if available {
            end / PAGE_SIZE
        } else {
            end.div_ceil(PAGE_SIZE)
        } as usize;
        if first >= last {
            return;
        }
        for word in first / 64..=(last - 1) / 64 {
            let low = first.saturating_sub(word * 64).min(64);
            let high = (last - word * 64).min(64);
            let mask = (u64::MAX << low) & (u64::MAX >> (64 - high));
            let before = self.free[word].count_ones() as usize;
            if available {
                self.free[word] |= mask;
            } else {
                self.free[word] &= !mask;
            }
            let after = self.free[word].count_ones() as usize;
            self.count = self.count - before + after;
        }
    }

    pub fn free_frames(&self) -> usize {
        self.count
    }

    pub fn allocate(&mut self) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        for step in 0..WORDS {
            let index = (self.cursor + step) % WORDS;
            let word = self.free[index];
            if word != 0 {
                let bit = word.trailing_zeros() as usize;
                self.free[index] &= !(1 << bit);
                self.owned[index] |= 1 << bit;
                self.count -= 1;
                self.cursor = index;
                return Some((index * 64 + bit) as u64 * PAGE_SIZE);
            }
        }
        None
    }

    /// `count` physically consecutive frames, for devices that read memory
    /// directly. Returns the first frame's address.
    pub fn allocate_run(&mut self, count: usize) -> Option<u64> {
        if count == 0 || count > self.count {
            return None;
        }
        let total = WORDS * 64;
        let free = |frame: usize| self.free[frame / 64] >> (frame % 64) & 1 != 0;
        let mut start = 0;
        while start + count <= total {
            match (start..start + count).find(|frame| !free(*frame)) {
                Some(taken) => start = taken + 1,
                None => {
                    for frame in start..start + count {
                        self.free[frame / 64] &= !(1 << (frame % 64));
                        self.owned[frame / 64] |= 1 << (frame % 64);
                    }
                    self.count -= count;
                    return Some(start as u64 * PAGE_SIZE);
                }
            }
        }
        None
    }

    pub fn release(&mut self, address: u64) -> Result<(), Error> {
        if address % PAGE_SIZE != 0 {
            return Err(Error::Unaligned);
        }
        let frame = address / PAGE_SIZE;
        if frame >= WORDS as u64 * 64 {
            return Err(Error::NotAllocated);
        }
        let index = frame as usize / 64;
        let mask = 1u64 << (frame as usize % 64);
        if self.owned[index] & mask == 0 {
            return Err(Error::NotAllocated);
        }
        self.owned[index] &= !mask;
        self.free[index] |= mask;
        self.count += 1;
        self.cursor = index;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(start: u64, length: u64, kind: u32) -> MemoryEntry {
        MemoryEntry {
            base: start,
            length,
            kind,
            attributes: 1,
        }
    }
    #[test]
    fn overlap_and_partial_reserved_pages_never_become_allocations() {
        let mut a = FrameAllocator::<2>::empty();
        let map = [
            entry(2 * PAGE_SIZE + 10, 1, 2),
            entry(0, 8 * PAGE_SIZE, 1),
            entry(0, 8 * PAGE_SIZE, 1),
        ];
        a.initialize(
            &map,
            &[Region {
                start: 0,
                end: PAGE_SIZE + 1,
            }],
        )
        .unwrap();
        assert_eq!(a.free_frames(), 5);
        for expected in 3..8 {
            assert_eq!(a.allocate(), Some(expected * PAGE_SIZE));
        }
        assert_eq!(a.allocate(), None);
    }
    #[test]
    fn only_whole_usable_pages_are_freed_and_disabled_entries_are_ignored() {
        let mut a = FrameAllocator::<1>::empty();
        let mut disabled = entry(0, 64 * PAGE_SIZE, 1);
        disabled.attributes = 0;
        a.initialize(&[disabled, entry(PAGE_SIZE + 1, 3 * PAGE_SIZE - 2, 1)], &[])
            .unwrap();
        assert_eq!(a.free_frames(), 1);
        assert_eq!(a.allocate(), Some(2 * PAGE_SIZE));
    }
    #[test]
    fn ownership_prevents_freeing_reserved_and_already_freed_frames() {
        let mut a = FrameAllocator::<1>::empty();
        a.initialize(
            &[entry(0, 4 * PAGE_SIZE, 1)],
            &[Region {
                start: 0,
                end: PAGE_SIZE,
            }],
        )
        .unwrap();
        assert_eq!(a.release(0), Err(Error::NotAllocated));
        let f = a.allocate().unwrap();
        assert_eq!(a.release(f + 1), Err(Error::Unaligned));
        a.release(f).unwrap();
        assert_eq!(a.release(f), Err(Error::NotAllocated));
        assert_eq!(a.allocate(), Some(f));
        assert_eq!(a.free_frames(), 2);
    }
    #[test]
    fn runs_skip_gaps_and_are_owned() {
        let mut a = FrameAllocator::<2>::empty();
        a.initialize(
            &[entry(0, 100 * PAGE_SIZE, 1), entry(5 * PAGE_SIZE, PAGE_SIZE, 2)],
            &[],
        )
        .unwrap();
        assert_eq!(a.allocate_run(6), Some(6 * PAGE_SIZE));
        assert_eq!(a.allocate_run(5), Some(0));
        assert_eq!(a.free_frames(), 99 - 11);
        a.release(8 * PAGE_SIZE).unwrap();
        assert_eq!(a.allocate_run(100), None);
        assert_eq!(a.allocate_run(80), Some(12 * PAGE_SIZE));
    }
    #[test]
    fn invalid_map_cannot_expose_frames() {
        let mut a = FrameAllocator::<1>::empty();
        let map = [entry(0, PAGE_SIZE, 1), entry(u64::MAX - 2, 8, 1)];
        assert_eq!(a.initialize(&map, &[]), Err(Error::InvalidMap));
        assert_eq!(a.allocate(), None);
    }
    #[test]
    fn physical_limit_clips_high_ram_without_wrapping() {
        let mut a = FrameAllocator::<1>::empty();
        a.initialize(
            &[
                entry(63 * PAGE_SIZE, 10 * PAGE_SIZE, 1),
                entry(1u64 << 40, PAGE_SIZE, 1),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(a.free_frames(), 1);
        assert_eq!(a.allocate(), Some(63 * PAGE_SIZE));
        assert_eq!(a.allocate(), None);
    }
}
