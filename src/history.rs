//! Undo / redo as whole-document snapshots. A to-do list is small, so copying
//! both lists per step is cheaper and far simpler than recording inverse edits.

use crate::model::Item;

const LIMIT: usize = 100;

#[derive(Clone)]
pub struct Snapshot {
    pub tasks: Vec<Item>,
    pub archive: Vec<Item>,
    pub label: String,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    /// Records the state *before* a change.
    pub fn record(&mut self, tasks: &[Item], archive: &[Item], label: &str) {
        if self.undo.len() == LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(Snapshot {
            tasks: tasks.to_vec(),
            archive: archive.to_vec(),
            label: label.into(),
        });
        self.redo.clear();
    }

    /// Returns the state to restore; `current` becomes the redo step.
    pub fn undo(&mut self, current: (Vec<Item>, Vec<Item>)) -> Option<Snapshot> {
        let snap = self.undo.pop()?;
        self.redo.push(Snapshot {
            tasks: current.0,
            archive: current.1,
            label: snap.label.clone(),
        });
        Some(snap)
    }

    pub fn redo(&mut self, current: (Vec<Item>, Vec<Item>)) -> Option<Snapshot> {
        let snap = self.redo.pop()?;
        self.undo.push(Snapshot {
            tasks: current.0,
            archive: current.1,
            label: snap.label.clone(),
        });
        Some(snap)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str) -> Item {
        Item {
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn undo_redo_round_trip_and_new_change_clears_redo() {
        let mut h = History::default();
        let v1 = vec![item("a")];
        let v2 = vec![item("a"), item("b")];
        h.record(&v1, &[], "Add task");
        let s = h.undo((v2.clone(), vec![])).unwrap();
        assert_eq!(s.tasks, v1);
        assert_eq!(h.redo_label(), Some("Add task"));
        let r = h.redo((v1.clone(), vec![])).unwrap();
        assert_eq!(r.tasks, v2);
        h.undo((v2.clone(), vec![]));
        h.record(&v1, &[], "Edit");
        assert!(h.redo_label().is_none());
    }

    #[test]
    fn history_is_capped() {
        let mut h = History::default();
        for i in 0..150 {
            h.record(&[item(&i.to_string())], &[], "Edit");
        }
        let mut n = 0;
        while h.undo((vec![], vec![])).is_some() {
            n += 1;
        }
        assert_eq!(n, LIMIT);
    }
}
