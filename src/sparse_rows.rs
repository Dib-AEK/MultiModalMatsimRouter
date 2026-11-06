//! Reuses per-stop/per-trip frontiers while clearing only rows touched by the previous search.
use std::ops::{Index, IndexMut};

pub(crate) struct SparseRows<T> {
    rows: Vec<Vec<T>>,
    marked: Vec<bool>,
    pub active: Vec<usize>,
}
impl<T> Default for SparseRows<T> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            marked: Vec::new(),
            active: Vec::new(),
        }
    }
}
impl<T> SparseRows<T> {
    pub fn reset(&mut self, length: usize) {
        self.clear();
        self.rows.resize_with(length, Vec::new);
        self.marked.resize(length, false);
    }
    pub fn clear(&mut self) {
        for i in self.active.drain(..) {
            self.rows[i].clear();
            self.marked[i] = false;
        }
    }
}
impl<T> Index<usize> for SparseRows<T> {
    type Output = Vec<T>;
    fn index(&self, i: usize) -> &Self::Output {
        &self.rows[i]
    }
}
impl<T> IndexMut<usize> for SparseRows<T> {
    fn index_mut(&mut self, i: usize) -> &mut Self::Output {
        if !self.marked[i] {
            self.marked[i] = true;
            self.active.push(i);
        }
        &mut self.rows[i]
    }
}
