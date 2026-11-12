//! Snapshot based undo / redo.

const LIMIT: usize = 200;

pub struct History<T> {
    undo: Vec<T>,
    redo: Vec<T>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self { undo: vec![], redo: vec![] }
    }
}

impl<T: Clone + PartialEq> History<T> {
    /// Record the state *before* a modification.
    pub fn push(&mut self, before: &T) {
        if self.undo.last() == Some(before) {
            return;
        }
        self.undo.push(before.clone());
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: &mut T) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(current, prev));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, current: &mut T) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(current, next));
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
