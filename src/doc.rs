//! The document: active lines and archived lines, and every operation on them.
//! No Windows code here, so all of it is unit-tested.

use crate::model::{Item, Kind, ListId, MAX_INDENT};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    List(ListId),
    Archive,
}

pub struct Doc {
    /// Next and Long-term lines; display order is the order in this vector.
    pub tasks: Vec<Item>,
    /// Archived lines, newest last.
    pub archive: Vec<Item>,
    next_id: u64,
}

impl Doc {
    pub fn new(tasks: Vec<Item>, archive: Vec<Item>) -> Self {
        let mut doc = Self {
            tasks,
            archive,
            next_id: 1,
        };
        doc.repair_ids();
        doc
    }

    /// Gives every item a unique non-zero id (hand-edited files may lack them).
    fn repair_ids(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let max = self
            .tasks
            .iter()
            .chain(&self.archive)
            .map(|i| i.id)
            .max()
            .unwrap_or(0);
        self.next_id = max + 1;
        for item in self.tasks.iter_mut().chain(self.archive.iter_mut()) {
            if item.id == 0 || !seen.insert(item.id) {
                item.id = self.next_id;
                seen.insert(item.id);
                self.next_id += 1;
            }
            item.indent = item.indent.min(MAX_INDENT);
        }
    }

    pub fn ids(&self, view: View) -> Vec<u64> {
        match view {
            View::List(l) => self
                .tasks
                .iter()
                .filter(|i| i.list == l)
                .map(|i| i.id)
                .collect(),
            View::Archive => self.archive.iter().rev().map(|i| i.id).collect(),
        }
    }

    pub fn get(&self, id: u64) -> Option<&Item> {
        self.tasks
            .iter()
            .find(|i| i.id == id)
            .or_else(|| self.archive.iter().find(|i| i.id == id))
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.tasks.iter_mut().find(|i| i.id == id)
    }

    pub fn contains_active(&self, id: u64) -> bool {
        self.tasks.iter().any(|i| i.id == id)
    }

    pub fn add(&mut self, list: ListId, kind: Kind, text: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(Item {
            id,
            kind,
            text: text.to_string(),
            list,
            ..Default::default()
        });
        id
    }

    pub fn remove(&mut self, ids: &[u64]) {
        self.tasks.retain(|i| !ids.contains(&i.id));
    }

    /// Saves edited text. An emptied task is deleted; an emptied text line stays
    /// as a blank spacer. Returns true if the line was deleted.
    pub fn set_text(&mut self, id: u64, text: &str) -> bool {
        let clean: String = text
            .replace("\r\n", " ")
            .replace(['\r', '\n'], " ")
            .trim()
            .to_string();
        let Some(item) = self.get_mut(id) else {
            return false;
        };
        if clean.is_empty() && item.kind == Kind::Task {
            self.remove(&[id]);
            return true;
        }
        item.text = clean;
        false
    }

    pub fn apply(&mut self, ids: &[u64], mut f: impl FnMut(&mut Item)) {
        for item in self.tasks.iter_mut().filter(|i| ids.contains(&i.id)) {
            f(item);
        }
    }

    pub fn indent(&mut self, ids: &[u64], delta: i8) {
        self.apply(ids, |i| {
            i.indent = (i.indent as i8 + delta).clamp(0, MAX_INDENT as i8) as u8
        });
    }

    /// Moves `group` (in its current display order) next to `target`.
    pub fn move_group(&mut self, group: &[u64], target: u64, after: bool) {
        if group.contains(&target) || group.is_empty() {
            return;
        }
        let moving: Vec<Item> = self
            .tasks
            .iter()
            .filter(|i| group.contains(&i.id))
            .cloned()
            .collect();
        self.tasks.retain(|i| !group.contains(&i.id));
        let Some(pos) = self.tasks.iter().position(|i| i.id == target) else {
            self.tasks.extend(moving);
            return;
        };
        let at = if after { pos + 1 } else { pos };
        self.tasks.splice(at..at, moving);
    }

    /// Moves lines to the end of another list, keeping their order and indent.
    pub fn move_to_list(&mut self, group: &[u64], list: ListId) {
        let mut moving: Vec<Item> = self
            .tasks
            .iter()
            .filter(|i| group.contains(&i.id))
            .cloned()
            .collect();
        if moving.is_empty() {
            return;
        }
        self.tasks.retain(|i| !group.contains(&i.id));
        for item in &mut moving {
            item.list = list;
        }
        self.tasks.extend(moving);
    }

    /// Moves the selected lines one position up or down within their list.
    pub fn nudge(&mut self, list: ListId, group: &[u64], up: bool) -> bool {
        let order = self.ids(View::List(list));
        let sel: Vec<u64> = order
            .iter()
            .copied()
            .filter(|id| group.contains(id))
            .collect();
        if sel.is_empty() {
            return false;
        }
        if up {
            let first = order.iter().position(|id| *id == sel[0]).unwrap();
            let Some(&prev) = order[..first].iter().rev().find(|id| !group.contains(id)) else {
                return false;
            };
            self.move_group(&sel, prev, false);
        } else {
            let last = order
                .iter()
                .position(|id| *id == *sel.last().unwrap())
                .unwrap();
            let Some(&next) = order[last + 1..].iter().find(|id| !group.contains(id)) else {
                return false;
            };
            self.move_group(&sel, next, true);
        }
        true
    }

    /// Archives tasks (text lines are never archived). Returns how many moved.
    pub fn archive(&mut self, ids: &[u64], today: &str) -> usize {
        let mut moved = 0;
        let mut keep = Vec::with_capacity(self.tasks.len());
        for mut item in std::mem::take(&mut self.tasks) {
            if ids.contains(&item.id) && item.kind == Kind::Task {
                item.archived_on = Some(today.to_string());
                self.archive.push(item);
                moved += 1;
            } else {
                keep.push(item);
            }
        }
        self.tasks = keep;
        moved
    }

    pub fn finished_ids(&self) -> Vec<u64> {
        self.tasks
            .iter()
            .filter(|i| i.kind == Kind::Task && i.done)
            .map(|i| i.id)
            .collect()
    }

    /// Back to the end of the list it came from.
    pub fn restore(&mut self, id: u64) {
        if let Some(pos) = self.archive.iter().position(|i| i.id == id) {
            let mut item = self.archive.remove(pos);
            item.archived_on = None;
            self.tasks.push(item);
        }
    }

    pub fn purge(&mut self, id: u64) {
        self.archive.retain(|i| i.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Doc {
        let mut d = Doc::new(Vec::new(), Vec::new());
        for (t, l) in [
            ("a", ListId::Next),
            ("b", ListId::Next),
            ("x", ListId::Later),
            ("c", ListId::Next),
            ("d", ListId::Next),
        ] {
            d.add(l, Kind::Task, t);
        }
        d
    }

    fn texts(d: &Doc, l: ListId) -> Vec<String> {
        d.ids(View::List(l))
            .iter()
            .map(|id| d.get(*id).unwrap().text.clone())
            .collect()
    }

    fn id(d: &Doc, t: &str) -> u64 {
        d.tasks.iter().find(|i| i.text == t).unwrap().id
    }

    #[test]
    fn move_group_after_and_before() {
        let mut d = doc();
        let (a, c, dd) = (id(&d, "a"), id(&d, "c"), id(&d, "d"));
        d.move_group(&[a, c], dd, true);
        assert_eq!(texts(&d, ListId::Next), ["b", "d", "a", "c"]);
        d.move_group(&[dd], a, false);
        assert_eq!(texts(&d, ListId::Next), ["b", "d", "a", "c"]);
        let b = id(&d, "b");
        d.move_group(&[b], c, true);
        assert_eq!(texts(&d, ListId::Next), ["d", "a", "c", "b"]);
        assert_eq!(texts(&d, ListId::Later), ["x"]);
    }

    #[test]
    fn move_onto_itself_is_a_no_op() {
        let mut d = doc();
        let a = id(&d, "a");
        d.move_group(&[a], a, true);
        assert_eq!(texts(&d, ListId::Next), ["a", "b", "c", "d"]);
    }

    #[test]
    fn move_to_other_list_appends_in_order() {
        let mut d = doc();
        let (b, dd) = (id(&d, "b"), id(&d, "d"));
        d.move_to_list(&[dd, b], ListId::Later);
        assert_eq!(texts(&d, ListId::Later), ["x", "b", "d"]);
        assert_eq!(texts(&d, ListId::Next), ["a", "c"]);
    }

    #[test]
    fn nudge_skips_over_selection_and_stops_at_edges() {
        let mut d = doc();
        let (a, c) = (id(&d, "a"), id(&d, "c"));
        assert!(!d.nudge(ListId::Next, &[a], true));
        assert!(d.nudge(ListId::Next, &[c], true));
        assert_eq!(texts(&d, ListId::Next), ["a", "c", "b", "d"]);
        assert!(d.nudge(ListId::Next, &[a, c], false));
        assert_eq!(texts(&d, ListId::Next), ["b", "a", "c", "d"]);
    }

    #[test]
    fn lines_are_independent() {
        let mut d = doc();
        let (a, b) = (id(&d, "a"), id(&d, "b"));
        d.indent(&[b], 1);
        d.apply(&[a], |i| {
            i.done = true;
            i.bold = true;
        });
        let bi = d.get(b).unwrap();
        assert!(!bi.done && !bi.bold && bi.indent == 1);
        d.archive(&[a], "2026-10-01");
        assert!(d.contains_active(b));
        assert_eq!(d.get(b).unwrap().indent, 1);
    }

    #[test]
    fn indent_is_clamped() {
        let mut d = doc();
        let a = id(&d, "a");
        for _ in 0..10 {
            d.indent(&[a], 1);
        }
        assert_eq!(d.get(a).unwrap().indent, MAX_INDENT);
        for _ in 0..10 {
            d.indent(&[a], -1);
        }
        assert_eq!(d.get(a).unwrap().indent, 0);
    }

    #[test]
    fn emptied_task_is_deleted_emptied_text_line_stays() {
        let mut d = doc();
        let a = id(&d, "a");
        let t = d.add(ListId::Next, Kind::Text, "Heading");
        assert!(d.set_text(a, "   "));
        assert!(!d.contains_active(a));
        assert!(!d.set_text(t, ""));
        assert_eq!(d.get(t).unwrap().text, "");
        assert!(!d.set_text(t, "two\r\nlines"));
        assert_eq!(d.get(t).unwrap().text, "two lines");
    }

    #[test]
    fn archive_finished_skips_text_lines_and_restore_returns_to_its_list() {
        let mut d = doc();
        let x = id(&d, "x");
        let t = d.add(ListId::Next, Kind::Text, "Heading");
        d.apply(&[x, t], |i| i.done = true);
        let fin = d.finished_ids();
        assert_eq!(fin, vec![x]);
        assert_eq!(d.archive(&fin, "2026-10-01"), 1);
        assert!(d.contains_active(t));
        assert_eq!(d.ids(View::Archive), vec![x]);
        assert_eq!(d.get(x).unwrap().archived_on.as_deref(), Some("2026-10-01"));
        d.restore(x);
        assert_eq!(texts(&d, ListId::Later), ["x"]);
        assert!(d.get(x).unwrap().archived_on.is_none());
        d.archive(&[x], "2026-10-01");
        d.purge(x);
        assert!(d.get(x).is_none());
    }

    #[test]
    fn duplicate_and_missing_ids_are_repaired() {
        let items = vec![
            Item {
                id: 5,
                text: "a".into(),
                ..Default::default()
            },
            Item {
                id: 5,
                text: "b".into(),
                ..Default::default()
            },
            Item {
                id: 0,
                text: "c".into(),
                indent: 9,
                ..Default::default()
            },
        ];
        let d = Doc::new(items, Vec::new());
        let ids: Vec<u64> = d.tasks.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![5, 6, 7]);
        assert_eq!(d.tasks[2].indent, MAX_INDENT);
    }
}
