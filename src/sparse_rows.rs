//! Maps touched stop/trip IDs to a compact reusable frontier pool, without dense vector headers.
use std::ops::{Index, IndexMut};
pub(crate) type Row<T> = Vec<T>;
const NONE: u32 = u32::MAX;
pub(crate) struct SparseRows<T> {
    slots: Vec<u32>,
    rows: Vec<Row<T>>,
    empty: Row<T>,
    pub active: Vec<usize>,
}
impl<T> Default for SparseRows<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            rows: Vec::new(),
            empty: Row::new(),
            active: Vec::new(),
        }
    }
}
impl<T> SparseRows<T> {
    pub fn reset(&mut self, length: usize) {
        self.clear();
        self.slots.resize(length, NONE);
    }
    pub fn clear(&mut self) {
        for i in self.active.drain(..) {
            self.rows[self.slots[i] as usize].clear();
            self.slots[i] = NONE;
        }
    }
}
impl<T> Index<usize> for SparseRows<T> {
    type Output = Row<T>;
    fn index(&self, i: usize) -> &Self::Output {
        if self.slots[i] == NONE {
            &self.empty
        } else {
            &self.rows[self.slots[i] as usize]
        }
    }
}
impl<T> IndexMut<usize> for SparseRows<T> {
    fn index_mut(&mut self, i: usize) -> &mut Self::Output {
        if self.slots[i] == NONE {
            let slot = self.active.len();
            self.slots[i] = u32::try_from(slot).expect("frontier index exceeds compact limits");
            if slot == self.rows.len() {
                self.rows.push(Row::new());
            }
            self.active.push(i);
        }
        &mut self.rows[self.slots[i] as usize]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reuses_slots_without_leaking_rows_between_ids_or_rounds() {
        let mut rows = SparseRows::<u32>::default();
        rows.reset(10000);
        rows[9000].push(7);
        rows[4].push(8);
        assert_eq!(rows.active, vec![9000, 4]);
        assert!(rows[20].is_empty());
        rows.clear();
        assert!(rows[9000].is_empty());
        rows[20].push(3);
        assert_eq!(rows[20].as_slice(), &[3]);
        assert_eq!(rows.rows.len(), 2);
        rows[20].extend(10..110); // reuse a grown frontier on a different ID
        rows.clear();
        rows[2].push(5);
        assert_eq!(rows[2].as_slice(), &[5]);
        rows.reset(3);
        assert!(rows[0].is_empty());
        rows[2].push(5);
        assert_eq!(rows[2].as_slice(), &[5]);
    }
}
