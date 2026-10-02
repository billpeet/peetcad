//! Undo and redo.
//!
//! Every change to the document is a named step. A step stores the model as it was before
//! the change; because the model shares its features between snapshots
//! ([`crate::Model`] keeps them behind shared pointers), a step costs a few pointers plus
//! the features that actually changed. Derived data (bodies, meshes) is not stored: after
//! an undo the model is rebuilt, and the features the undo didn't touch come from the
//! engine's cache.

/// One undoable step.
#[derive(Clone, Debug)]
struct Step<T> {
    label: String,
    /// The state before the change (on the undo stack) or after it (on the redo stack).
    state: T,
    /// Steps recorded in a row with the same merge key become one (dragging a value).
    merge: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct History<T> {
    undo: Vec<Step<T>>,
    redo: Vec<Step<T>>,
    limit: usize,
    /// Whether the newest undo step may still absorb changes with the same merge key.
    open: bool,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self::new(200)
    }
}

impl<T> History<T> {
    /// A history keeping at most `limit` steps.
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            open: false,
        }
    }

    /// Records a change: `before` is the state the change started from. Clears redo.
    pub fn record(&mut self, label: &str, before: T) {
        self.push(label, before, None);
    }

    /// Records a change that continues the previous one if that has the same `key` (a value
    /// being dragged, a name being typed): the two become one step. Call
    /// [`History::seal`] when the interaction ends.
    pub fn record_merging(&mut self, label: &str, key: u64, before: T) {
        if self.open && self.undo.last().is_some_and(|s| s.merge == Some(key)) {
            self.redo.clear();
            return;
        }
        self.push(label, before, Some(key));
    }

    fn push(&mut self, label: &str, state: T, merge: Option<u64>) {
        self.redo.clear();
        self.undo.push(Step {
            label: label.to_owned(),
            state,
            merge,
        });
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.open = merge.is_some();
    }

    /// Ends merging: the next change starts a new step whatever its key.
    pub fn seal(&mut self) {
        self.open = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What undo would undo ("Edit Extrude1").
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// Number of steps that can be undone.
    pub fn depth(&self) -> usize {
        self.undo.len()
    }

    /// Undoes the last step: `current` becomes the state before it. Returns its label.
    pub fn undo(&mut self, current: &mut T) -> Option<String> {
        let mut step = self.undo.pop()?;
        std::mem::swap(current, &mut step.state);
        self.open = false;
        let label = step.label.clone();
        self.redo.push(step);
        Some(label)
    }

    /// Redoes the last undone step. Returns its label.
    pub fn redo(&mut self, current: &mut T) -> Option<String> {
        let mut step = self.redo.pop()?;
        std::mem::swap(current, &mut step.state);
        self.open = false;
        let label = step.label.clone();
        self.undo.push(step);
        Some(label)
    }

    /// Forgets everything (a new or freshly opened document).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_round_trip() {
        let mut h = History::new(10);
        let mut v = 1;
        h.record("to 2", v);
        v = 2;
        h.record("to 3", v);
        v = 3;
        assert_eq!(h.undo_label(), Some("to 3"));
        assert_eq!(h.undo(&mut v).as_deref(), Some("to 3"));
        assert_eq!(v, 2);
        assert_eq!(h.undo(&mut v).as_deref(), Some("to 2"));
        assert_eq!(v, 1);
        assert_eq!(h.undo(&mut v), None);
        assert_eq!(h.redo(&mut v).as_deref(), Some("to 2"));
        assert_eq!(h.redo(&mut v).as_deref(), Some("to 3"));
        assert_eq!(v, 3);
        assert!(!h.can_redo());
    }

    #[test]
    fn a_new_change_clears_redo() {
        let mut h = History::new(10);
        let mut v = 1;
        h.record("a", v);
        v = 2;
        h.undo(&mut v);
        h.record("b", v);
        v = 5;
        assert!(!h.can_redo());
        h.undo(&mut v);
        assert_eq!(v, 1);
    }

    #[test]
    fn merging_makes_one_step_until_sealed() {
        let mut h = History::new(10);
        let mut v = 0;
        for next in 1..=5 {
            h.record_merging("drag", 7, v);
            v = next;
        }
        assert_eq!(h.depth(), 1);
        h.seal();
        h.record_merging("drag", 7, v);
        v = 9;
        assert_eq!(h.depth(), 2);
        // A different key never merges.
        h.record_merging("other", 8, v);
        v = 10;
        assert_eq!(h.depth(), 3);
        h.undo(&mut v);
        h.undo(&mut v);
        assert_eq!(v, 5);
        h.undo(&mut v);
        assert_eq!(v, 0);
    }

    #[test]
    fn the_limit_drops_the_oldest_steps() {
        let mut h = History::new(3);
        let mut v = 0;
        for next in 1..=10 {
            h.record("step", v);
            v = next;
        }
        assert_eq!(h.depth(), 3);
        while h.undo(&mut v).is_some() {}
        assert_eq!(v, 7);
    }
}
